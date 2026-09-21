//! Shared provider usage policy for desktop and worker hosts.
use crate::remote::{BROWSERSTACK_SESSION_API, RemoteAdapterKind, RemoteProviderProfile};
use serde::Deserialize;
use std::time::Duration;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
/// Normalized shared usage; independent of the provider's wire format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderUsage {
    pub running: u64,
    pub allowed: u64,
    pub queued: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UsageError {
    #[error("Shared usage is not supported by this provider.")]
    Unsupported,
    #[error("Shared usage requires available provider credentials.")]
    Credentials,
    #[error("The provider refused access to shared usage.")]
    AccessDenied,
    #[error("Shared usage is temporarily unavailable.")]
    Unavailable,
    #[error("The provider returned an invalid usage response.")]
    InvalidResponse,
}

/// Provider-specific routing and response normalization stay behind this adapter.
#[derive(Clone, Copy)]
pub enum UsageAdapter {
    Browserstack,
}

impl UsageAdapter {
    #[must_use]
    pub fn for_profile(profile: &RemoteProviderProfile) -> Option<Self> {
        match profile.adapter {
            RemoteAdapterKind::Browserstack => Some(Self::Browserstack),
            RemoteAdapterKind::Webdriver => None,
        }
    }

    #[must_use]
    pub fn endpoint(self) -> String {
        match self {
            Self::Browserstack => format!("{BROWSERSTACK_SESSION_API}/automate/plan.json"),
        }
    }

    #[must_use]
    pub fn authorizes_origin(self, origin: &str) -> bool {
        match self {
            Self::Browserstack => matches!(
                origin,
                "https://hub-cloud.browserstack.com"
                    | "https://hub.browserstack.com"
                    | "https://hub-apse.browserstack.com"
                    | "https://hub-aps.browserstack.com"
                    | "https://hub-euw.browserstack.com"
                    | "https://hub-use.browserstack.com"
                    | "https://hub-usw.browserstack.com"
            ),
        }
    }

    /// # Errors
    /// Rejects malformed or unsupported provider usage responses.
    pub fn decode(self, bytes: &[u8]) -> Result<ProviderUsage, UsageError> {
        match self {
            Self::Browserstack => {
                #[derive(Deserialize)]
                struct Plan {
                    parallel_sessions_running: u64,
                    parallel_sessions_max_allowed: u64,
                    team_parallel_sessions_max_allowed: Option<u64>,
                    queued_sessions: u64,
                }
                let plan: Plan = serde_json::from_slice(bytes).map_err(|_| UsageError::InvalidResponse)?;
                Ok(ProviderUsage {
                    running: plan.parallel_sessions_running,
                    allowed: plan
                        .team_parallel_sessions_max_allowed
                        .map_or(plan.parallel_sessions_max_allowed, |team| {
                            team.min(plan.parallel_sessions_max_allowed)
                        }),
                    queued: plan.queued_sessions,
                })
            }
        }
    }
}

/// # Errors
/// Reports refused credentials, unavailable transport, or invalid bounded responses.
pub fn fetch_usage(adapter: UsageAdapter, endpoint: &str, authorization: &str) -> Result<ProviderUsage, UsageError> {
    let config = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build();
    let mut response = ureq::Agent::new_with_config(config)
        .get(endpoint)
        .header("Authorization", authorization)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| UsageError::Unavailable)?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(UsageError::AccessDenied),
        _ => return Err(UsageError::Unavailable),
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(|_| UsageError::InvalidResponse)?;
    adapter.decode(&bytes)
}
