//! Azure Resource Manager (ARM) client for CPU workers. Each operation owns one
//! resource group named from its operation id; a lost answer is reconciled by that
//! name and the group's `horizon-operation` tag, never by a second create.
//!
//! Every URL is built from validated segments with an explicit API version, and an
//! operation URL from ARM is polled only when it sits under this subscription. Every
//! answer is untrusted: bodies are size-limited, decoded into small typed views,
//! checked against the requested identity, and never echoed into errors except as a
//! sanitized [`Reason`]. A 409 is never retried and never a definite refusal: it is
//! the signal to reconcile.
use crate::{Cancellation, CloudError, Credential, Reason};
use serde::Deserialize;
use serde_json::Value;
use std::{io::Read, time::Duration};
use ureq::http::HeaderMap;

pub mod operation;
pub mod reconcile;
pub mod resource;
pub mod url;

#[cfg(test)]
mod tests;

pub use operation::{BACKOFF_MS, LongRunning, OperationRef};
pub use reconcile::{Found, RESOURCE_GROUP_PREFIX, WORKER_VM_NAME};
pub use resource::{DataDisk, Deployed, DeploymentState, Group, VirtualMachine};

use operation::operation_ref;
use resource::{deployment_state, group_info, vm_view};
use url::{valid_resource_group_name, valid_resource_name, valid_subscription_id};

/// The public ARM endpoint.
pub const MANAGEMENT_ENDPOINT: &str = "https://management.azure.com";
/// Explicit API versions; ARM never picks one for this client.
pub const RESOURCE_GROUP_API_VERSION: &str = "2022-09-01";
pub const DEPLOYMENT_API_VERSION: &str = "2022-09-01";
pub const COMPUTE_API_VERSION: &str = "2024-03-01";
/// The profile `provider` value that selects Azure.
pub const PROVIDER: &str = "azure";
/// Tag on every resource group this client's callers create, naming the operation
/// that owns it.
pub const OPERATION_TAG: &str = "horizon-operation";
/// The per-request budget.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest `Retry-After` this client honours. The value arrives in an untrusted
/// answer, so ARM alone must not decide how long a caller waits.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);
/// Longest wait for one ARM operation, waits and polls included.
pub const ACTION_TIMEOUT: Duration = Duration::from_mins(10);
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
    /// How the operation poll waits: a real sleep, or a recorder in tests.
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
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
    /// ARM refused for lack of capacity or quota, so it made nothing. Only a 400 may
    /// match a detail code, because a template deployment reports a capacity refusal
    /// that way. A 409 must match its own top-level code: a nested code can belong to
    /// an earlier operation, while the top-level code says another operation still
    /// holds the resource, so the state is unknown and a caller must reconcile.
    #[must_use]
    pub fn capacity(&self) -> bool {
        match self {
            Self::Provider {
                status: 400,
                code,
                details,
                ..
            } => capacity_codes(code, details),
            Self::Provider { status: 409, code, .. } => CAPACITY_CODES.contains(&code.as_str()),
            _ => false,
        }
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
        let capacity = failure.capacity();
        match failure {
            Failure::Local(error) => error,
            Failure::Provider { reason, .. } if capacity => Self::Capacity(reason),
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

/// The error code or one of its detail codes is a capacity refusal. Only a 400 is
/// asked, because a template deployment reports its refusal as a detail of
/// `InvalidTemplateDeployment`; on a 409 a nested code belongs to another operation.
fn capacity_codes(code: &str, details: &[String]) -> bool {
    std::iter::once(code)
        .chain(details.iter().map(String::as_str))
        .any(|code| CAPACITY_CODES.contains(&code))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Put,
}

/// A 2xx answer. A body that is empty or only whitespace is `Value::Null`.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: HeaderMap,
    pub(crate) value: Value,
}

/// The resource group that holds the resources of `operation_id`.
#[must_use]
pub fn resource_group_name(operation_id: &str) -> String {
    format!("{RESOURCE_GROUP_PREFIX}{operation_id}")
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
    /// credential can never be sent to another host this way, and operations are
    /// polled without waits.
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
        client.sleep = Box::new(|_| {});
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
            sleep: Box::new(std::thread::sleep),
        })
    }

    /// This client with its operation-poll waits given to `sleep` in place of a real
    /// sleep, so a test can record the schedule without waiting it out.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_sleeps(mut self, sleep: impl Fn(Duration) + Send + Sync + 'static) -> Self {
        self.sleep = Box::new(sleep);
        self
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

    /// Submits `template` with `parameters` as an incremental deployment `name` in
    /// `group`. ARM finishes a deployment later; poll [`Deployed::operation`].
    /// # Errors
    /// `Invalid` for a bad segment (nothing is sent), `InvalidResponse` for a running
    /// deployment without an operation URL under this subscription, `IdentityMismatch`
    /// for a body that names another deployment, or the provider failure. Only a
    /// [`Failure::definite`] failure proves that ARM made nothing.
    pub fn put_deployment(
        &self,
        group: &str,
        name: &str,
        template: &Value,
        parameters: &Value,
        cancel: &Cancellation,
    ) -> Result<Deployed, Failure> {
        let url = self.resource_url(group, "Microsoft.Resources/deployments", name, DEPLOYMENT_API_VERSION)?;
        let body = serde_json::json!({
            "properties": { "mode": "Incremental", "template": template, "parameters": parameters }
        });
        let reply = self.send_within(Method::Put, &url, Some(&body), cancel, None)?;
        // An asynchronous acceptance may carry no deployment body at all.
        let state = if reply.status == 202 && reply.value.pointer("/properties/provisioningState").is_none() {
            DeploymentState {
                provisioning_state: "Accepted".into(),
                outputs: std::collections::BTreeMap::new(),
            }
        } else {
            deployment_state(&reply.value, &self.subscription_id, group, name)?
        };
        let operation = match operation_ref(&reply.headers) {
            Some(operation) if self.owns_operation_url(operation.url()) => LongRunning::Accepted(operation),
            None if state.is_terminal() => LongRunning::Completed,
            _ => return Err(Failure::Local(CloudError::InvalidResponse)),
        };
        Ok(Deployed { state, operation })
    }

    /// Reads deployment `name` in `group`.
    /// # Errors
    /// As [`Self::put_deployment`], without the operation URL check.
    pub fn get_deployment(
        &self,
        group: &str,
        name: &str,
        cancel: &Cancellation,
    ) -> Result<Option<DeploymentState>, Failure> {
        let url = self.resource_url(group, "Microsoft.Resources/deployments", name, DEPLOYMENT_API_VERSION)?;
        self.get_json_within(&url, cancel, None)?
            .map(|value| deployment_state(&value, &self.subscription_id, group, name))
            .transpose()
    }

    /// Reads VM `name` in `group` with its instance view.
    /// # Errors
    /// `Invalid` for a bad segment (nothing is sent), `IdentityMismatch` for a body
    /// whose name, resource id or `vmId` does not fit the request, `InvalidResponse`
    /// for an unusable body, or the provider failure.
    pub fn get_vm(&self, group: &str, name: &str, cancel: &Cancellation) -> Result<Option<VirtualMachine>, Failure> {
        let url = self.resource_url(group, "Microsoft.Compute/virtualMachines", name, COMPUTE_API_VERSION)?;
        self.get_json_within(&format!("{url}&$expand=instanceView"), cancel, None)?
            .map(|value| vm_view(&value, &self.subscription_id, group, name))
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

    fn resource_url(&self, group: &str, provider_path: &str, name: &str, api_version: &str) -> Result<String, Failure> {
        if !valid_resource_group_name(group) {
            return Err(Failure::Local(CloudError::Invalid("Invalid Azure resource group name")));
        }
        if !valid_resource_name(name) {
            return Err(Failure::Local(CloudError::Invalid("Invalid Azure resource name")));
        }
        Ok(format!(
            "{}/subscriptions/{}/resourceGroups/{group}/providers/{provider_path}/{name}?api-version={api_version}",
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
        match self.send_within(Method::Get, url, None, cancel, budget) {
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
        body: Option<&Value>,
        cancel: &Cancellation,
        budget: Option<Duration>,
    ) -> Result<Reply, Failure> {
        cancel.check().map_err(Failure::Local)?;
        if budget.is_some_and(|budget| budget.is_zero()) {
            return Err(Failure::Local(CloudError::Transport));
        }
        let timeout = budget.map_or(REQUEST_TIMEOUT, |budget| budget.min(REQUEST_TIMEOUT));
        let auth = zeroize::Zeroizing::new(format!("Bearer {}", self.credential.value()));
        let empty = Value::Object(serde_json::Map::new());
        let response = match method {
            Method::Get => self
                .agent
                .get(url)
                .config()
                .timeout_global(Some(timeout))
                .build()
                .header("Authorization", auth.as_str())
                .call(),
            Method::Put => self
                .agent
                .put(url)
                .config()
                .timeout_global(Some(timeout))
                .build()
                .header("Authorization", auth.as_str())
                .send_json(body.unwrap_or(&empty)),
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
        let headers = response.headers().clone();
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
        Ok(Reply { status, headers, value })
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
        .map(|seconds| Duration::from_secs(seconds).min(RETRY_AFTER_CAP))
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
