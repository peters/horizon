//! Azure Resource Manager (ARM) client for CPU workers.
//!
//! Every URL is built from validated segments with an explicit API version. Every
//! answer is untrusted: bodies are size-limited, decoded into small typed views,
//! checked against the requested identity, and never echoed into errors except as a
//! sanitized [`Reason`]. A 409 is never retried and never a definite refusal: it is
//! the signal to reconcile.
use crate::{Cancellation, CloudError, Credential, Reason};
use serde::Deserialize;
use serde_json::Value;
use std::{io::Read, time::Duration};
use ureq::http::HeaderMap;

pub mod resource;
pub mod url;

#[cfg(test)]
mod tests;

pub use resource::Group;

use resource::group_info;
use url::{valid_resource_group_name, valid_subscription_id};

/// The public ARM endpoint.
pub const MANAGEMENT_ENDPOINT: &str = "https://management.azure.com";
/// Explicit API versions; ARM never picks one for this client.
pub const RESOURCE_GROUP_API_VERSION: &str = "2022-09-01";
/// The profile `provider` value that selects Azure.
pub const PROVIDER: &str = "azure";
/// Tag on every resource group this client's callers create, naming the operation
/// that owns it.
pub const OPERATION_TAG: &str = "horizon-operation";
/// The per-request budget.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// ureq refuses a body that reaches the limit, so the largest body read is one byte less.
const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;
/// Larger error bodies are not provider explanations worth parsing.
const FAILURE_BODY_LIMIT: u64 = 8 * 1024;

/// Error codes of a capacity refusal: ARM made nothing.
const CAPACITY_CODES: [&str; 4] = [
    "SkuNotAvailable",
    "QuotaExceeded",
    "ZonalAllocationFailed",
    "AllocationFailed",
];
/// 403 codes that mean the credential, not the request, is at fault.
const AUTHENTICATION_CODES: [&str; 3] = [
    "AuthenticationFailed",
    "AuthorizationFailed",
    "ExpiredAuthenticationToken",
];

/// A client for one subscription with one bearer credential.
pub struct Azure {
    agent: ureq::Agent,
    credential: Credential,
    subscription_id: String,
    endpoint: String,
}

/// A request that failed, keeping ARM's error codes so callers can tell capacity,
/// conflicts and throttles apart before it becomes a [`CloudError`].
#[derive(Debug)]
pub enum Failure {
    /// Nothing reached ARM, or its answer was unusable.
    Local(CloudError),
    /// ARM answered with a status outside 2xx.
    Provider {
        /// The HTTP status of the answer.
        status: u16,
        /// `error.code` of the body, or empty.
        code: String,
        /// The `code` of each `error.details` entry.
        details: Vec<String>,
        /// `error.message`, sanitized; empty for a body that is too large or repeats the credential.
        reason: Reason,
        /// The `Retry-After` of the answer, such as the wait a 429 asks for.
        retry_after: Option<Duration>,
    },
}

impl Failure {
    /// ARM refused for lack of capacity or quota, so it made nothing.
    #[must_use]
    pub fn capacity(&self) -> bool {
        matches!(self, Self::Provider { status: 400 | 409, code, details, .. }
            if capacity_codes(code, details))
    }

    /// ARM has no resource at the URL.
    #[must_use]
    pub fn not_found(&self) -> bool {
        matches!(self, Self::Provider { status: 404, .. })
    }

    /// Another operation holds the resource, or a lock or deletion is in progress.
    /// The state is unknown until it is reconciled.
    #[must_use]
    pub fn conflict(&self) -> bool {
        matches!(self, Self::Provider { status: 409, .. }) && !self.capacity()
    }

    /// ARM refused before it acted, so nothing was made or changed. A 409, a 5xx, a
    /// transport failure and an unusable 2xx are not definite.
    #[must_use]
    pub fn definite(&self) -> bool {
        match self {
            Self::Local(error) => matches!(error, CloudError::Cancelled | CloudError::Invalid(_)),
            Self::Provider { status, .. } => self.capacity() || matches!(status, 400 | 401 | 403 | 429),
        }
    }
}

impl From<Failure> for CloudError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Local(error) => error,
            Failure::Provider {
                status: 400 | 409,
                code,
                details,
                reason,
                ..
            } if capacity_codes(&code, &details) => Self::Capacity(reason),
            Failure::Provider { status: 401, .. } => Self::Unauthorized,
            Failure::Provider { status: 403, code, .. } if AUTHENTICATION_CODES.contains(&code.as_str()) => {
                Self::Unauthorized
            }
            Failure::Provider {
                status: 400 | 403,
                reason,
                ..
            } => Self::Rejected(reason),
            Failure::Provider { status: 500..=599, .. } => Self::Transport,
            Failure::Provider { status, reason, .. } => Self::Http(status, reason),
        }
    }
}

/// The error code or one of its detail codes is a capacity refusal; a template
/// deployment reports one as a detail of `InvalidTemplateDeployment`.
fn capacity_codes(code: &str, details: &[String]) -> bool {
    std::iter::once(code)
        .chain(details.iter().map(String::as_str))
        .any(|code| CAPACITY_CODES.contains(&code))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
}

/// A 2xx answer. A body that is empty or only whitespace is `Value::Null`.
pub(crate) struct Reply {
    pub(crate) value: Value,
}

impl Azure {
    /// A client for `subscription_id` on the public ARM endpoint, over HTTPS only,
    /// without redirects.
    /// # Errors
    /// `Invalid` for a subscription id that is not a lowercase UUID.
    pub fn new(credential: Credential, subscription_id: &str) -> Result<Self, CloudError> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .https_only(true)
            .build();
        Self::with_agent(ureq::Agent::new_with_config(config), credential, subscription_id)
    }

    /// A client for a fake ARM endpoint on this machine, so callers in other crates
    /// can test against scripted answers. Only a loopback address is accepted, so a
    /// credential can never be sent to another host this way.
    /// # Errors
    /// `Invalid` for an address that is not loopback or a malformed subscription id.
    #[doc(hidden)]
    pub fn loopback(
        credential: Credential,
        subscription_id: &str,
        address: std::net::SocketAddr,
    ) -> Result<Self, CloudError> {
        if !address.ip().is_loopback() {
            return Err(CloudError::Invalid("A test Azure endpoint must be a loopback address"));
        }
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        let mut client = Self::with_agent(ureq::Agent::new_with_config(config), credential, subscription_id)?;
        client.endpoint = format!("http://{address}");
        Ok(client)
    }

    /// A client that sends through `agent` to the public ARM endpoint. Crate-private:
    /// the agent carries the HTTPS-only and no-redirect rules, so only `new`, `loopback`
    /// and tests choose it.
    pub(crate) fn with_agent(
        agent: ureq::Agent,
        credential: Credential,
        subscription_id: &str,
    ) -> Result<Self, CloudError> {
        if !valid_subscription_id(subscription_id) {
            return Err(CloudError::Invalid("Invalid Azure subscription id"));
        }
        Ok(Self {
            agent,
            credential,
            subscription_id: subscription_id.to_string(),
            endpoint: MANAGEMENT_ENDPOINT.to_string(),
        })
    }

    /// The one subscription every request of this client addresses.
    #[must_use]
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    #[cfg(test)]
    fn config(&self) -> &ureq::config::Config {
        self.agent.config()
    }

    /// Reads a resource group.
    /// # Errors
    /// `Invalid` for a bad group name (nothing is sent), `IdentityMismatch` for a body
    /// that names another group, `InvalidResponse` for an unusable body, or the
    /// provider failure.
    pub fn get_resource_group(&self, name: &str, cancel: &Cancellation) -> Result<Option<Group>, Failure> {
        let url = self.group_url(name)?;
        self.get_json_within(&url, cancel, None)?
            .map(|value| group_info(&value, &self.subscription_id, name))
            .transpose()
    }

    fn group_url(&self, name: &str) -> Result<String, Failure> {
        if !valid_resource_group_name(name) {
            return Err(Failure::Local(CloudError::Invalid("Invalid Azure resource group name")));
        }
        Ok(format!(
            "{}/subscriptions/{}/resourceGroups/{name}?api-version={RESOURCE_GROUP_API_VERSION}",
            self.endpoint, self.subscription_id
        ))
    }

    /// A GET whose 404 is `Ok(None)`.
    pub(crate) fn get_json_within(
        &self,
        url: &str,
        cancel: &Cancellation,
        budget: Option<Duration>,
    ) -> Result<Option<Value>, Failure> {
        match self.send_within(Method::Get, url, cancel, budget) {
            Ok(reply) => Ok(Some(reply.value)),
            Err(failure) if failure.not_found() => Ok(None),
            Err(failure) => Err(failure),
        }
    }

    /// Sends one request to the absolute `url`. A `budget` covers the whole exchange,
    /// is capped at `REQUEST_TIMEOUT`, and a zero budget sends nothing.
    pub(crate) fn send_within(
        &self,
        method: Method,
        url: &str,
        cancel: &Cancellation,
        budget: Option<Duration>,
    ) -> Result<Reply, Failure> {
        cancel.check().map_err(Failure::Local)?;
        if budget.is_some_and(|budget| budget.is_zero()) {
            return Err(Failure::Local(CloudError::Transport));
        }
        let timeout = budget.map_or(REQUEST_TIMEOUT, |budget| budget.min(REQUEST_TIMEOUT));
        let auth = zeroize::Zeroizing::new(format!("Bearer {}", self.credential.value()));
        let response = match method {
            Method::Get => self
                .agent
                .get(url)
                .config()
                .timeout_global(Some(timeout))
                .build()
                .header("Authorization", auth.as_str())
                .call(),
        };
        self.answer(response)
    }

    /// The status, headers and body of a response, as `send_within` reports them.
    fn answer(&self, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Reply, Failure> {
        let mut response = response.map_err(|_| Failure::Local(CloudError::Transport))?;
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(self.failure(status, &mut response));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(RESPONSE_LIMIT)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => Failure::Local(CloudError::InvalidResponse),
                _ => Failure::Local(CloudError::Transport),
            })?;
        let value = if body.iter().all(u8::is_ascii_whitespace) {
            Value::Null
        } else {
            serde_json::from_slice(&body).map_err(|_| Failure::Local(CloudError::InvalidResponse))?
        };
        Ok(Reply { value })
    }

    /// A failed or oversized read yields no reason rather than masking the status.
    fn failure(&self, status: u16, response: &mut ureq::http::Response<ureq::Body>) -> Failure {
        #[derive(Deserialize)]
        struct Envelope {
            error: Detail,
        }
        #[derive(Deserialize)]
        struct Detail {
            #[serde(default)]
            code: String,
            #[serde(default)]
            message: String,
            #[serde(default)]
            details: Vec<Code>,
        }
        #[derive(Deserialize)]
        struct Code {
            #[serde(default)]
            code: String,
        }
        let retry_after = retry_after(response.headers());
        let mut body = Vec::new();
        let read = response
            .body_mut()
            .as_reader()
            .take(FAILURE_BODY_LIMIT + 1)
            .read_to_end(&mut body);
        // A body cut off by the request budget is a transport failure, not a
        // provider answer: the status alone says nothing about what happened.
        if read.as_ref().is_err_and(io_timed_out) {
            return Failure::Local(CloudError::Transport);
        }
        let detail = (read.is_ok() && body.len() as u64 <= FAILURE_BODY_LIMIT)
            .then(|| {
                serde_json::from_slice::<Envelope>(&body)
                    .map(|envelope| envelope.error)
                    .or_else(|_| serde_json::from_slice::<Detail>(&body))
                    .ok()
            })
            .flatten();
        let (code, details, reason) = detail.map_or_else(
            || (String::new(), Vec::new(), Reason::default()),
            |detail| {
                (
                    detail.code,
                    detail.details.into_iter().map(|detail| detail.code).collect(),
                    self.reason(&detail.message),
                )
            },
        );
        Failure::Provider {
            status,
            code,
            details,
            reason,
            retry_after,
        }
    }

    /// A provider explanation that never repeats the credential.
    fn reason(&self, message: &str) -> Reason {
        Reason::from_text(message, self.credential.value())
    }
}

/// `Retry-After` in whole seconds. ARM does not send the HTTP-date form, so that form
/// gives `None`.
#[must_use]
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn ureq_timed_out(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::Timeout(_) => true,
        ureq::Error::Io(error) => io_timed_out(error),
        _ => false,
    }
}

/// Body reads surface a ureq timeout as an I/O error that wraps it.
fn io_timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) || error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .is_some_and(ureq_timed_out)
}
