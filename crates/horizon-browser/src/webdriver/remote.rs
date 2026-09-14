//! Remote session ownership: one allocation at a hosted grid, a hard
//! lifetime and idle watchdog on its own thread, and a bounded, verified
//! release. Allocation is never retried after an ambiguous result, and no
//! mutation is replayed.

mod watchdog;

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::http::HttpError;
use super::remote_http::{RemoteAuthorizationHeader, RemoteHttpClient};
use super::session::handshake::{NewSession, parse_new_session_response};
use super::transport::ClassicTransport;
use watchdog::Watchdog;

/// Bounded per-attempt wait for `DELETE /session/{id}` during release.
const RELEASE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);
/// Total release attempts for one owned session id.
const RELEASE_ATTEMPTS: u8 = 3;

/// Everything the driver needs to open one session at a remote endpoint.
/// Provider adaptation (device requirements to capabilities, limits from
/// configuration, the resolved authorization) happens before this point.
#[derive(Clone)]
pub struct RemoteSessionRequest {
    /// Control endpoint including base path, already validated by the host.
    pub endpoint: String,
    /// Header sent to that endpoint only; `None` for an unauthenticated grid.
    pub authorization: Option<Arc<RemoteAuthorizationHeader>>,
    /// `alwaysMatch` capabilities. No credential may be placed here.
    pub capabilities: Value,
    /// Bound for New Session; slower than any local browser launch.
    pub allocation_timeout: Duration,
    /// Local hard lifetime: the session is released when it elapses.
    pub max_session: Duration,
    /// Released after this long without a user or agent command.
    pub idle_release: Duration,
    /// Configured target name for display and audit.
    pub label: String,
}

impl fmt::Debug for RemoteSessionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSessionRequest")
            .field("endpoint", &self.endpoint)
            .field("authorized", &self.authorization.is_some())
            .field("allocation_timeout", &self.allocation_timeout)
            .field("max_session", &self.max_session)
            .field("idle_release", &self.idle_release)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Why a remote session could not start. Nothing here carries a credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteStartFailure {
    /// The endpoint was rejected before any request was sent.
    InvalidEndpoint(String),
    /// The provider answered New Session with a `WebDriver` error.
    AllocationFailed { error: String, message: String },
    /// No trustworthy answer: the provider may have allocated a device.
    AllocationUnknown { reason: String },
}

impl fmt::Display for RemoteStartFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(reason) => write!(formatter, "remote endpoint rejected: {reason}"),
            Self::AllocationFailed { error, message } => {
                write!(formatter, "remote session not created ({error}): {message}")
            }
            Self::AllocationUnknown { reason } => write!(
                formatter,
                "remote allocation outcome unknown ({reason}); not retried because the provider may hold a device"
            ),
        }
    }
}

/// Why the watchdog ended a session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteExpiry {
    HardDeadline,
    Idle,
}

/// What release established at the provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteReleaseOutcome {
    /// The provider accepted the delete.
    Released,
    /// The provider no longer knows the session.
    AlreadyGone,
    /// Every bounded attempt ended without a trustworthy answer.
    ReleaseUnknown { attempts: u8, reason: String },
    /// The provider refused the delete.
    Failed { error: String, message: String },
}

/// Lifecycle facts the host surfaces alongside the usual browser events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteSessionEvent {
    Allocating {
        label: String,
    },
    Allocated {
        label: String,
        session_digest: String,
    },
    AllocationUnknown {
        label: String,
        reason: String,
    },
    /// The provider refused, or the endpoint was rejected locally. Terminal
    /// for the lifecycle; the reason never carries a credential.
    AllocationFailed {
        label: String,
        reason: String,
    },
    Expired {
        label: String,
        reason: RemoteExpiry,
    },
    Released {
        label: String,
        outcome: RemoteReleaseOutcome,
    },
}

/// The driver's view of a remote grid: transport plus the local watchdog,
/// which starts only once a session exists.
pub(super) struct RemoteHost {
    transport: Arc<RemoteHttpClient>,
    label: String,
    max_session: Duration,
    idle_release: Duration,
    watchdog: Option<Watchdog>,
}

impl RemoteHost {
    /// Bind the transport. Nothing is sent yet.
    ///
    /// # Errors
    /// [`RemoteStartFailure::InvalidEndpoint`].
    pub(super) fn connect(request: &RemoteSessionRequest) -> Result<Self, RemoteStartFailure> {
        let authorization = match &request.authorization {
            Some(header) => Some(clone_header(header)?),
            None => None,
        };
        let transport = RemoteHttpClient::new(&request.endpoint, authorization)
            .map_err(|error| RemoteStartFailure::InvalidEndpoint(error.to_string()))?;
        Ok(Self {
            transport: Arc::new(transport),
            label: request.label.clone(),
            max_session: request.max_session,
            idle_release: request.idle_release,
            watchdog: None,
        })
    }

    pub(super) fn transport(&self) -> &dyn ClassicTransport {
        self.transport.as_ref()
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    /// One New Session request under the allocation timeout. A transport
    /// failure or an unusable success is `AllocationUnknown`; only a complete
    /// `WebDriver` error is `AllocationFailed`. Never retried. On success the
    /// watchdog starts, so the allocation wait counts against neither the
    /// hard lifetime nor the idle policy.
    ///
    /// # Errors
    /// See [`RemoteStartFailure`].
    pub(super) fn allocate(&mut self, request: &RemoteSessionRequest) -> Result<NewSession, RemoteStartFailure> {
        let body = json!({ "capabilities": { "alwaysMatch": request.capabilities } });
        let session = match self
            .transport
            .post_with_read_timeout("/session", &body, request.allocation_timeout)
        {
            Ok(response) => parse_new_session_response(&response)
                .map_err(|reason| RemoteStartFailure::AllocationUnknown { reason })?,
            Err(HttpError::WebDriver { error, message }) => {
                return Err(RemoteStartFailure::AllocationFailed { error, message });
            }
            Err(other) => {
                return Err(RemoteStartFailure::AllocationUnknown {
                    reason: other.to_string(),
                });
            }
        };
        self.watchdog = Some(Watchdog::start(
            Arc::clone(&self.transport),
            session.id.clone(),
            self.max_session,
            self.idle_release,
            Instant::now(),
        ));
        Ok(session)
    }

    /// A user or agent command happened. Frame polling never calls this.
    pub(super) fn note_activity(&mut self, now: Instant) {
        if let Some(watchdog) = &self.watchdog {
            watchdog.note_activity(now);
        }
    }

    /// The watchdog's verdict: the hard deadline or the idle policy elapsed
    /// and the session is being, or has been, released. Sticky.
    pub(super) fn check_expiry(&self) -> Option<RemoteExpiry> {
        self.watchdog.as_ref().and_then(Watchdog::expired)
    }

    /// Release the exact owned session. When the watchdog already released
    /// it, its settled outcome is returned and nothing is sent again.
    pub(super) fn release(&mut self, session_id: &str) -> RemoteReleaseOutcome {
        if let Some(outcome) = self.watchdog.take().and_then(Watchdog::finish) {
            return outcome;
        }
        release_session(&self.transport, session_id)
    }
}

/// Delete one session with bounded retries. A `WebDriver` answer settles the
/// outcome on the first attempt; transport failures are retried up to the
/// bound, and the last reason is reported.
fn release_session(transport: &RemoteHttpClient, session_id: &str) -> RemoteReleaseOutcome {
    let path = format!("/session/{session_id}");
    let mut last_reason = String::new();
    for attempt in 1..=RELEASE_ATTEMPTS {
        match transport.request("DELETE", &path, None, RELEASE_ATTEMPT_TIMEOUT) {
            Ok(_) => return RemoteReleaseOutcome::Released,
            Err(HttpError::WebDriver { error, message }) => {
                if error == "invalid session id" {
                    return RemoteReleaseOutcome::AlreadyGone;
                }
                return RemoteReleaseOutcome::Failed { error, message };
            }
            Err(other) => {
                last_reason = other.to_string();
                tracing::warn!(attempt, "remote session release attempt failed: {last_reason}");
            }
        }
    }
    RemoteReleaseOutcome::ReleaseUnknown {
        attempts: RELEASE_ATTEMPTS,
        reason: last_reason,
    }
}

impl fmt::Debug for RemoteHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteHost")
            .field("label", &self.label)
            .field("origin", &self.transport.origin())
            .field("expired", &self.check_expiry())
            .finish_non_exhaustive()
    }
}

/// The transport owns its header; the request keeps a shared one so the
/// session config stays cloneable. Copy the value into a fresh sealed header.
fn clone_header(header: &RemoteAuthorizationHeader) -> Result<RemoteAuthorizationHeader, RemoteStartFailure> {
    RemoteAuthorizationHeader::new(header.value().to_string())
        .map_err(|error| RemoteStartFailure::InvalidEndpoint(error.to_string()))
}

#[cfg(test)]
mod tests;
