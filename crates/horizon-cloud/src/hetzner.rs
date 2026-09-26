//! Hetzner Cloud REST adapter for CPU workers on x86 servers. Never repeats a
//! create request after an uncertain response; a lost response is reconciled
//! through the operation label. Hetzner has no hourly GPUs, and a volume can
//! only attach to servers in the location that created it.
use crate::{Cancellation, CloudError, Credential, Reason};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    io::Read,
    time::{Duration, Instant},
};

pub mod catalog;
pub mod servers;
pub mod volumes;

#[cfg(test)]
mod tests;

/// Larger error bodies are not provider explanations worth parsing.
const FAILURE_BODY_LIMIT: u64 = 8 * 1024;
const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;
/// The per-request budget.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Label on every resource this adapter creates, naming the operation that owns it.
pub const OPERATION_LABEL: &str = "horizon-operation";
/// The largest page Hetzner serves.
const PAGE_SIZE: u32 = 50;
/// Bounds pagination against a provider that never reports a last page.
const MAX_PAGES: u32 = 100;
/// Longest wait for a provider action such as a create, attach or delete.
const ACTION_TIMEOUT: Duration = Duration::from_mins(5);

pub struct Hetzner {
    agent: ureq::Agent,
    credential: Credential,
    endpoint: String,
    poll: Duration,
}

/// A request that failed, keeping Hetzner's error code so callers can tell
/// capacity refusals and name conflicts apart before it becomes a `CloudError`.
#[derive(Debug)]
pub(crate) enum Failure {
    Local(CloudError),
    Provider { status: u16, code: String, reason: Reason },
}
impl Failure {
    /// The requested server type is not available in that location right now.
    pub(crate) fn capacity(&self) -> bool {
        matches!(self, Self::Provider { code, .. } if code == "resource_unavailable" || code == "placement_error")
    }
    pub(crate) fn not_found(&self) -> bool {
        matches!(self, Self::Provider { status: 404, .. })
    }
    /// Another resource already has the requested name.
    pub(crate) fn name_taken(&self) -> bool {
        matches!(self, Self::Provider { code, .. } if code == "uniqueness_error")
    }
    /// The provider refused before acting, so nothing was created.
    pub(crate) fn definite(&self) -> bool {
        match self {
            Self::Local(error) => matches!(error, CloudError::Cancelled),
            Self::Provider { status, .. } => matches!(status, 400 | 401 | 403 | 412 | 422 | 429),
        }
    }
}
impl From<Failure> for CloudError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Local(error) => error,
            Failure::Provider {
                status: 403,
                code,
                reason,
            } if code == "resource_limit_exceeded" => Self::Rejected(reason),
            // Hetzner refuses changes during maintenance; the credential is fine.
            Failure::Provider {
                status: 403,
                code,
                reason,
            } if code == "maintenance" => Self::Http(403, reason),
            Failure::Provider { status: 401 | 403, .. } => Self::Unauthorized,
            Failure::Provider {
                status: 400 | 412 | 422,
                reason,
                ..
            } => Self::Rejected(reason),
            Failure::Provider { status, reason, .. } => Self::Http(status, reason),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Action {
    pub id: u64,
    pub status: String,
    #[serde(default)]
    pub error: Option<ActionError>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ActionError {
    #[serde(default)]
    pub message: String,
}
#[derive(Deserialize)]
struct ActionEnvelope {
    action: Action,
}
#[derive(Deserialize)]
struct Meta {
    pagination: Pagination,
}
#[derive(Deserialize)]
struct Pagination {
    next_page: Option<u32>,
}

impl Hetzner {
    #[must_use]
    pub fn new(credential: Credential) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            credential,
            endpoint: "https://api.hetzner.cloud/v1".into(),
            poll: Duration::from_secs(2),
        }
    }

    /// Every item of a paginated collection. `query` holds the filters, without paging.
    pub(crate) fn list_all<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &str,
        key: &str,
        cancel: &Cancellation,
    ) -> Result<Vec<T>, Failure> {
        let mut items = Vec::new();
        let mut page = 1;
        for _ in 0..MAX_PAGES {
            let separator = if query.is_empty() { "" } else { "&" };
            let mut value = self.send(
                "GET",
                &format!("{path}?{query}{separator}page={page}&per_page={PAGE_SIZE}"),
                None,
                cancel,
            )?;
            let batch: Vec<T> = serde_json::from_value(value[key].take()).map_err(|_| invalid())?;
            items.extend(batch);
            let meta: Meta = serde_json::from_value(value["meta"].take()).map_err(|_| invalid())?;
            match meta.pagination.next_page {
                Some(next) if next > page => page = next,
                Some(_) => return Err(invalid()),
                None => return Ok(items),
            }
        }
        Err(invalid())
    }

    /// Waits until `action` finishes. A failed action is a refusal carrying its message.
    pub(crate) fn wait(&self, action: &Action, cancel: &Cancellation) -> Result<(), CloudError> {
        let deadline = Instant::now() + ACTION_TIMEOUT;
        let mut current = action.clone();
        loop {
            match current.status.as_str() {
                "success" => return Ok(()),
                "error" => {
                    let message = current.error.map(|error| error.message).unwrap_or_default();
                    return Err(CloudError::Rejected(Reason::from_text(
                        &message,
                        self.credential.value(),
                    )));
                }
                "running" => {}
                _ => return Err(CloudError::InvalidResponse),
            }
            if Instant::now() >= deadline {
                return Err(CloudError::Invalid(
                    "Provider action is still running; check again later",
                ));
            }
            std::thread::sleep(self.poll);
            cancel.check()?;
            let envelope: ActionEnvelope =
                serde_json::from_value(self.send("GET", &format!("/actions/{}", current.id), None, cancel)?)
                    .map_err(|_| CloudError::InvalidResponse)?;
            if envelope.action.id != current.id {
                return Err(CloudError::InvalidResponse);
            }
            current = envelope.action;
        }
    }

    /// Sends one request. Success without a body is `Value::Null`.
    pub(crate) fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        cancel: &Cancellation,
    ) -> Result<Value, Failure> {
        cancel.check().map_err(Failure::Local)?;
        let url = format!("{}{path}", self.endpoint);
        let auth = zeroize::Zeroizing::new(format!("Bearer {}", self.credential.value()));
        let response = match method {
            "POST" => self
                .agent
                .post(&url)
                .header("Authorization", auth.as_str())
                .send_json(body.unwrap_or_else(|| Value::Object(serde_json::Map::new()))),
            "DELETE" => self.agent.delete(&url).header("Authorization", auth.as_str()).call(),
            _ => self.agent.get(&url).header("Authorization", auth.as_str()).call(),
        };
        let mut response = response.map_err(|_| Failure::Local(CloudError::Transport))?;
        let status = response.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(self.failure(status, &mut response));
        }
        if status == 204 {
            return Ok(Value::Null);
        }
        response
            .body_mut()
            .with_config()
            .limit(RESPONSE_LIMIT)
            .read_json()
            .map_err(|_| invalid())
    }

    /// A failed or oversized read yields no reason rather than masking the status.
    fn failure(&self, status: u16, response: &mut ureq::http::Response<ureq::Body>) -> Failure {
        #[derive(Deserialize)]
        struct Body {
            error: Detail,
        }
        #[derive(Deserialize)]
        struct Detail {
            #[serde(default)]
            code: String,
            #[serde(default)]
            message: String,
        }
        let mut body = Vec::new();
        let read = response
            .body_mut()
            .as_reader()
            .take(FAILURE_BODY_LIMIT + 1)
            .read_to_end(&mut body);
        let detail = (read.is_ok() && body.len() as u64 <= FAILURE_BODY_LIMIT)
            .then(|| serde_json::from_slice::<Body>(&body).ok())
            .flatten()
            .map(|body| body.error);
        let (code, reason) = detail.map_or_else(
            || (String::new(), Reason::default()),
            |detail| (detail.code, Reason::from_text(&detail.message, self.credential.value())),
        );
        Failure::Provider { status, code, reason }
    }
}

fn invalid() -> Failure {
    Failure::Local(CloudError::InvalidResponse)
}

/// The name and label value of the resources an operation owns. Hetzner names
/// are host names, so only lowercase letters, digits and inner hyphens are accepted.
/// # Errors
/// Refuses operation IDs that cannot name a server.
pub fn resource_name(operation_id: &str) -> Result<String, CloudError> {
    let valid = !operation_id.is_empty()
        && operation_id.len() <= 48
        && operation_id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !operation_id.starts_with('-')
        && !operation_id.ends_with('-');
    if !valid {
        return Err(CloudError::Invalid(
            "Hetzner operation IDs use lowercase letters, digits and inner hyphens, at most 48 characters",
        ));
    }
    Ok(format!("horizon-cloud-{operation_id}"))
}

/// A provider name such as a server type or location, safe to place in a URL or body.
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
