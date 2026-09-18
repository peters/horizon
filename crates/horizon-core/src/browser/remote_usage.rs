//! Provider-wide capacity snapshots. Informational only: allocation is authoritative.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use horizon_browser::remote::{
    BROWSERSTACK_SESSION_API, CredentialStoreKind, RemoteAdapterKind, RemoteProviderProfile,
};
use serde::Deserialize;

use crate::remote_browser_credential::{
    CredentialLocator, CredentialStores, CredentialWorkbench, KeyringCredentialStore, ProviderAuthorization,
    RemoteCredentialError, RemoteCredentialStore, Sealed, SecretSink, SessionCredentialStore, StoreOpener,
    resolve_authorization,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_USAGE_WORKERS: usize = 32;
static USAGE_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct UsageWorkerPermit<'a>(&'a AtomicUsize);
impl<'a> UsageWorkerPermit<'a> {
    fn acquire(counter: &'a AtomicUsize) -> Option<Self> {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < MAX_USAGE_WORKERS).then(|| count + 1)
            })
            .ok()
            .map(|_| Self(counter))
    }
}
impl Drop for UsageWorkerPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

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
enum UsageAdapter {
    Browserstack,
}

impl UsageAdapter {
    fn for_profile(profile: &RemoteProviderProfile) -> Option<Self> {
        match profile.adapter {
            RemoteAdapterKind::Browserstack => Some(Self::Browserstack),
            RemoteAdapterKind::Webdriver => None,
        }
    }

    fn endpoint(self) -> String {
        match self {
            Self::Browserstack => format!("{BROWSERSTACK_SESSION_API}/automate/plan.json"),
        }
    }

    fn authorizes_origin(self, origin: &str) -> bool {
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

    fn decode(self, bytes: &[u8]) -> Result<ProviderUsage, UsageError> {
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

/// One configured provider's last successful snapshot and bounded refresh worker.
#[derive(Default)]
pub struct ProviderUsageMonitor {
    profile: Option<RemoteProviderProfile>,
    pending: Option<Receiver<Result<(ProviderUsage, Instant), UsageError>>>,
    last_attempt: Option<Instant>,
    pub sample: Option<(ProviderUsage, Instant)>,
    pub error: Option<UsageError>,
}

impl ProviderUsageMonitor {
    #[must_use]
    pub fn supported(profile: &RemoteProviderProfile) -> bool {
        UsageAdapter::for_profile(profile).is_some()
    }

    #[must_use]
    pub fn refreshing(&self) -> bool {
        self.pending.is_some()
    }

    #[must_use]
    pub fn next_refresh_in(&self) -> Duration {
        self.last_attempt
            .map_or(Duration::ZERO, |at| REFRESH_INTERVAL.saturating_sub(at.elapsed()))
    }

    /// Called by active usage consumers. Network and OS-store reads run
    /// off the render thread. A changed profile drops results from the old binding.
    pub fn update(&mut self, profile: &RemoteProviderProfile, credentials: &CredentialWorkbench, force: bool) {
        if self.profile.as_ref() != Some(profile) {
            *self = Self::default();
            self.profile = Some(profile.clone());
        }
        self.poll();
        if self.refreshing() || (!force && self.last_attempt.is_some_and(|at| at.elapsed() < REFRESH_INTERVAL)) {
            return;
        }
        self.last_attempt = Some(Instant::now());
        let Some(adapter) = UsageAdapter::for_profile(profile) else {
            self.error = Some(UsageError::Unsupported);
            return;
        };
        let prepared = match PreparedUsage::new(profile, credentials) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let Some(permit) = UsageWorkerPermit::acquire(&USAGE_WORKERS) else {
            self.error = Some(UsageError::Unavailable);
            return;
        };
        let (sender, receiver) = channel();
        match std::thread::Builder::new().name("remote-usage".into()).spawn(move || {
            // Native credential reads may outlive the consumer's deadline. Keep
            // their permit until the worker exits, without holding allocation locks.
            let _permit = permit;
            let _ = sender.send(prepared.fetch(adapter).map(|usage| (usage, Instant::now())));
        }) {
            Ok(_) => self.pending = Some(receiver),
            Err(_) => self.error = Some(UsageError::Unavailable),
        }
    }

    fn poll(&mut self) {
        let Some(pending) = &self.pending else { return };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) if self.last_attempt.is_none_or(|at| at.elapsed() < REQUEST_TIMEOUT) => return,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Err(UsageError::Unavailable),
        };
        self.pending = None;
        match result {
            Ok(sample) => {
                self.sample = Some(sample);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

struct PreparedUsage {
    profile: RemoteProviderProfile,
    memory: SessionCredentialStore,
    keychain: Option<StoreOpener>,
}

impl PreparedUsage {
    fn new(profile: &RemoteProviderProfile, credentials: &CredentialWorkbench) -> Result<Self, UsageError> {
        let mut snapshot = Self {
            profile: profile.clone(),
            memory: SessionCredentialStore::new(),
            keychain: None,
        };
        // Copy only this provider's in-memory credentials. OS-store access uses
        // an independent connection on the worker, never the allocation store.
        for reference in profile.authentication.references() {
            let binding = profile
                .credential_bindings
                .get(reference)
                .ok_or(UsageError::Credentials)?;
            let store: &dyn RemoteCredentialStore = match binding.store {
                CredentialStoreKind::Session => credentials.session_store(),
                CredentialStoreKind::Environment => credentials.environment_store(),
                CredentialStoreKind::OsKeychain => {
                    snapshot.keychain = Some(Box::new(|| {
                        KeyringCredentialStore::open()
                            .map(|store| Box::new(store) as Box<dyn RemoteCredentialStore + Send>)
                    }));
                    continue;
                }
            };
            let locator = CredentialLocator::new(&profile.endpoint, reference, binding);
            store
                .with_secret(
                    &locator,
                    &mut SnapshotSink {
                        memory: &mut snapshot.memory,
                        locator: &locator,
                    },
                )
                .map_err(|_| UsageError::Credentials)?;
            if let Some(binding) = snapshot.profile.credential_bindings.get_mut(reference) {
                binding.store = CredentialStoreKind::Session;
            }
        }
        Ok(snapshot)
    }

    fn authorization(mut self) -> Result<ProviderAuthorization, UsageError> {
        let keychain = self
            .keychain
            .take()
            .map(|open| open())
            .transpose()
            .map_err(|_| UsageError::Credentials)?;
        resolve_authorization(
            &self.profile,
            &CredentialStores {
                session: &self.memory,
                environment: None,
                os_keychain: keychain.as_deref().map(|store| -> &dyn RemoteCredentialStore { store }),
            },
        )
        .map_err(|_| UsageError::Credentials)?
        .ok_or(UsageError::Credentials)
    }

    fn fetch(self, adapter: UsageAdapter) -> Result<ProviderUsage, UsageError> {
        if !adapter.authorizes_origin(&self.profile.endpoint.origin()) {
            return Err(UsageError::Unsupported);
        }
        let authorization = self.authorization()?;
        if !adapter.authorizes_origin(authorization.origin()) {
            return Err(UsageError::Credentials);
        }
        // Only explicitly trusted provider hubs may delegate their credentials
        // to this fixed provider API. Redirects never extend that authorization.
        fetch_usage(adapter, &adapter.endpoint(), authorization.header_value())
    }
}

struct SnapshotSink<'a> {
    memory: &'a mut SessionCredentialStore,
    locator: &'a CredentialLocator,
}
impl Sealed for SnapshotSink<'_> {}
impl SecretSink for SnapshotSink<'_> {
    fn accept(&mut self, bytes: &[u8]) -> Result<(), RemoteCredentialError> {
        self.memory.put(self.locator, bytes)
    }
}

fn fetch_usage(adapter: UsageAdapter, endpoint: &str, authorization: &str) -> Result<ProviderUsage, UsageError> {
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

#[cfg(test)]
mod tests;
