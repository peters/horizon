//! ARM long-running operations. A mutation that ARM finishes later answers with an
//! `Azure-AsyncOperation` URL; this module reads that header, proves the URL sits
//! under this client's subscription, and polls it exactly as ARM sent it. The poll
//! loop is bounded by `ACTION_TIMEOUT`, follows `BACKOFF_MS`, and waits at least as
//! long as ARM's `Retry-After` asks, so a throttled poll waits instead of failing.
use super::{ACTION_TIMEOUT, Azure, Failure, Method, resource::text, retry_after};
use crate::{Cancellation, CloudError};
use std::time::{Duration, Instant};
use ureq::http::HeaderMap;

/// The waits between operation polls, in milliseconds. After the last step, every
/// further wait repeats it until `ACTION_TIMEOUT` ends the loop.
pub const BACKOFF_MS: [u64; 9] = [500, 1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000, 30_000];
/// Longest operation URL polled. ARM signs its operation URLs with a certificate in
/// the query string (`c=`) of about 3 KB.
const OPERATION_URL_LIMIT: usize = 8_192;
const ASYNC_OPERATION_HEADER: &str = "azure-asyncoperation";

/// How ARM answered a mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LongRunning {
    /// The resource reached a terminal state in the answer itself.
    Completed,
    /// ARM continues the work; poll the operation for its end.
    Accepted(OperationRef),
}

/// An `Azure-AsyncOperation` URL and the first wait ARM asked for. Only
/// [`operation_ref`] makes one, and [`Azure::await_operation`] checks the URL again
/// before it sends anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationRef {
    url: String,
    retry_after: Option<Duration>,
}

impl OperationRef {
    /// The operation URL as ARM returned it.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The `Retry-After` of the answer that returned the URL.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }
}

/// The `Azure-AsyncOperation` URL of an answer and its `Retry-After`. A `Location`
/// header alone uses another protocol and gives `None`.
#[must_use]
pub fn operation_ref(headers: &HeaderMap) -> Option<OperationRef> {
    let url = headers.get(ASYNC_OPERATION_HEADER)?.to_str().ok()?;
    Some(OperationRef {
        url: url.to_string(),
        retry_after: retry_after(headers),
    })
}

/// The time left before `deadline`, or `None` when it has passed.
#[must_use]
pub fn remaining(deadline: Instant) -> Option<Duration> {
    remaining_at(Instant::now(), deadline)
}

/// The time left before `deadline` at `now`, or `None` when it has passed.
#[must_use]
pub fn remaining_at(now: Instant, deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(now);
    (!left.is_zero()).then_some(left)
}

impl Azure {
    /// Whether `url` is an operation under this client's subscription on its endpoint
    /// that no URL normalisation can move: each path segment is a plain token (no dot
    /// segments, percent-encoding, backslashes, `@` or empty segments) and the query
    /// holds only the token characters of ARM's signed parameters.
    #[must_use]
    pub fn owns_operation_url(&self, url: &str) -> bool {
        let prefix = format!("{}/subscriptions/{}/", self.endpoint, self.subscription_id);
        let Some(rest) = url.strip_prefix(&prefix) else {
            return false;
        };
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let plain_segment = |segment: &str| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        };
        url.len() <= OPERATION_URL_LIMIT
            && path.split('/').all(plain_segment)
            && query
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'=' | b'&'))
    }

    /// Polls `operation` until it ends. Each wait is the next `BACKOFF_MS` step, made
    /// longer by the last `Retry-After` and shorter by what is left of
    /// `ACTION_TIMEOUT`; each poll gets only what is left as its request budget. A
    /// throttled (429) poll waits and continues.
    /// # Errors
    /// `InvalidResponse` for an operation URL outside this subscription (nothing is
    /// sent), a poll that answers 404, or a status without a `status` field.
    /// `Rejected` with ARM's reason for a `Failed` or `Canceled` operation.
    /// `Invalid` when the operation still runs at the bound, `Cancelled` when
    /// `cancel` fires, and the mapped [`Failure`] of any other failed poll.
    pub fn await_operation(&self, operation: &OperationRef, cancel: &Cancellation) -> Result<(), CloudError> {
        if !self.owns_operation_url(&operation.url) {
            return Err(CloudError::InvalidResponse);
        }
        cancel.check()?;
        let started = Instant::now();
        let deadline = started + ACTION_TIMEOUT;
        // A recorded wait passes no time, so the bound also counts the waits the
        // loop asked for. A real wait passes at least that much time.
        let mut waited = Duration::ZERO;
        let now = |waited: Duration| Instant::now().max(started + waited);
        let mut retry = operation.retry_after;
        let steps = BACKOFF_MS
            .iter()
            .copied()
            .chain(BACKOFF_MS.last().copied().into_iter().cycle());
        for step in steps {
            let Some(left) = remaining_at(now(waited), deadline) else {
                break;
            };
            let delay = Duration::from_millis(step).max(retry.unwrap_or_default()).min(left);
            (self.sleep)(delay);
            waited += delay;
            cancel.check()?;
            let Some(budget) = remaining_at(now(waited), deadline) else {
                break;
            };
            let reply = match self.send_within(Method::Get, &operation.url, None, cancel, Some(budget)) {
                Ok(reply) => reply,
                Err(Failure::Provider {
                    status: 429,
                    retry_after,
                    ..
                }) => {
                    retry = retry_after;
                    continue;
                }
                Err(Failure::Provider { status: 404, .. }) => return Err(CloudError::InvalidResponse),
                Err(failure) => return Err(failure.into()),
            };
            retry = retry_after(&reply.headers);
            match text(&reply.value, "/status").as_deref() {
                Some("Succeeded") => return Ok(()),
                Some("Failed" | "Canceled") => {
                    return Err(CloudError::Rejected(
                        self.reason(&text(&reply.value, "/error/message").unwrap_or_default()),
                    ));
                }
                Some(_) => {}
                None => return Err(CloudError::InvalidResponse),
            }
        }
        Err(CloudError::Invalid(
            "Provider operation is still running; check again later",
        ))
    }
}
