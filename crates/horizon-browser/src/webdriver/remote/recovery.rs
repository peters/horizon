//! Private exact-allocation recovery; no provider identifiers are serialized.
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::super::remote_http::RemoteHttpClient;
use super::RemoteReleaseOutcome;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRecoveryStatus {
    #[default]
    InUse,
    Unresolved,
    Reconciling,
    Active,
    Released,
    IdentityUnavailable,
    AuthenticationRequired,
    ProviderUnavailable,
}

impl RemoteRecoveryStatus {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InUse => "The local driver has not finished; close the panel first.",
            Self::Unresolved => "Release is unconfirmed. Reconcile this allocation to check its exact session.",
            Self::Reconciling => "Checking the exact session at the original provider.",
            Self::Active => "The provider still reports this session as active; capacity remains held.",
            Self::Released => "The exact allocation is released; its capacity is available.",
            Self::IdentityUnavailable => {
                "No exact session identity was received; capacity remains held. Contact the provider with the original allocation details."
            }
            Self::AuthenticationRequired => {
                "The original credential was rejected; capacity remains held. Restore its access at the original provider before reconciling again."
            }
            Self::ProviderUnavailable => {
                "The provider gave no trustworthy session result; capacity remains held. Retry when the original provider is available."
            }
        }
    }
}

#[derive(Clone)]
pub struct RemoteAllocation {
    reference: String,
    state: Arc<Mutex<State>>,
}

/// Host authorization captured atomically before its live manifest disappears.
#[derive(Clone, Debug)]
pub struct RemoteAllocationScope {
    pub host: String,
    pub workspace: Option<String>,
    pub owner: Option<String>,
}

#[derive(Default)]
struct State {
    published: bool,
    scope: Option<RemoteAllocationScope>,
    identity: Option<(Arc<RemoteHttpClient>, String)>,
    retired: bool,
    status: RemoteRecoveryStatus,
}

impl Default for RemoteAllocation {
    fn default() -> Self {
        Self {
            reference: crate::new_action_id(),
            state: Arc::default(),
        }
    }
}

impl fmt::Debug for RemoteAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteAllocation")
            .field("reference", &self.reference)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl PartialEq for RemoteAllocation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}
impl Eq for RemoteAllocation {}

impl RemoteAllocation {
    /// Called inside the coordinator's manifest publication transaction.
    pub fn mark_published(&self) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.published = true;
        state.scope = None;
    }

    /// Called under the manifest deletion lock, after checking driver ownership.
    pub fn retain_scope(&self, scope: RemoteAllocationScope) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scope = Some(scope);
    }

    /// Scope authorization and the returned status come from one snapshot.
    #[must_use]
    pub fn status_for(&self, host: &str, owner: &str, workspace: &str, admitted: bool) -> Option<RemoteRecoveryStatus> {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        permits(
            &state,
            &Access {
                host,
                owner,
                workspace,
                admitted,
            },
        )
        .then_some(state.status)
    }

    /// Authorize and admit a reconciliation under the same state lock.
    #[must_use]
    pub fn reconcile_for(&self, host: &str, owner: &str, workspace: &str, admitted: bool) -> bool {
        self.reconcile_authorized(Some(Access {
            host,
            owner,
            workspace,
            admitted,
        }))
    }

    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    #[must_use]
    pub fn status(&self) -> RemoteRecoveryStatus {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
    }

    #[must_use]
    pub fn is_released(&self) -> bool {
        self.status() == RemoteRecoveryStatus::Released
    }

    pub(super) fn identify(&self, transport: Arc<RemoteHttpClient>, session: String) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .identity = Some((transport, session));
    }

    pub(crate) fn finish(&self, outcome: Option<&RemoteReleaseOutcome>) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.retired = true;
        if matches!(
            outcome,
            Some(
                RemoteReleaseOutcome::Released
                    | RemoteReleaseOutcome::AlreadyGone
                    | RemoteReleaseOutcome::NeverAllocated
            )
        ) {
            state.status = RemoteRecoveryStatus::Released;
            state.identity = None;
        } else if state.status != RemoteRecoveryStatus::Released {
            state.status = if state.identity.is_some() {
                RemoteRecoveryStatus::Unresolved
            } else {
                RemoteRecoveryStatus::IdentityUnavailable
            };
        }
    }

    /// A host may abandon an admission only before it hands it to a driver.
    pub fn cancel_before_launch(&self) {
        self.finish(Some(&RemoteReleaseOutcome::NeverAllocated));
    }

    /// Starts at most one bounded, read-only probe. Safe to call repeatedly;
    /// active drivers and allocations without exact identities are untouched.
    pub fn reconcile(&self) {
        let _ = self.reconcile_authorized(None);
    }

    fn reconcile_authorized(&self, access: Option<Access<'_>>) -> bool {
        let identity = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if access.as_ref().is_some_and(|access| !permits(&state, access)) {
                return false;
            }
            if !state.retired
                || matches!(
                    state.status,
                    RemoteRecoveryStatus::Released | RemoteRecoveryStatus::Reconciling
                )
            {
                return true;
            }
            let Some(identity) = state.identity.clone() else {
                state.status = RemoteRecoveryStatus::IdentityUnavailable;
                return true;
            };
            state.status = RemoteRecoveryStatus::Reconciling;
            identity
        };
        let allocation = self.clone();
        if std::thread::Builder::new()
            .name("remote-reconcile".into())
            .spawn(move || {
                let (transport, session) = identity;
                let status = probe(&transport, &session);
                let mut state = allocation
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.status = status;
                if status == RemoteRecoveryStatus::Released {
                    state.identity = None;
                }
            })
            .is_err()
        {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .status = RemoteRecoveryStatus::ProviderUnavailable;
        }
        true
    }
}

#[derive(Clone, Copy)]
struct Access<'a> {
    host: &'a str,
    owner: &'a str,
    workspace: &'a str,
    admitted: bool,
}

fn permits(state: &State, access: &Access<'_>) -> bool {
    if state.published && (state.retired || state.scope.is_some()) {
        state.scope.as_ref().is_some_and(|scope| {
            scope.host == access.host
                && scope.owner.as_deref() == Some(access.owner)
                && scope.workspace.as_deref() == Some(access.workspace)
        })
    } else {
        access.admitted
    }
}

fn probe(transport: &RemoteHttpClient, session: &str) -> RemoteRecoveryStatus {
    let path = format!("/session/{session}/url");
    let Ok((status, bytes)) = transport.request_bytes("GET", &path, None, Duration::from_secs(10)) else {
        return RemoteRecoveryStatus::ProviderUnavailable;
    };
    if matches!(status, 401 | 403) {
        return RemoteRecoveryStatus::AuthenticationRequired;
    }
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return RemoteRecoveryStatus::ProviderUnavailable;
    };
    if status == 404 && body["value"]["error"] == "invalid session id" {
        RemoteRecoveryStatus::Released
    } else if status == 200 && body["value"].is_string() {
        RemoteRecoveryStatus::Active
    } else {
        RemoteRecoveryStatus::ProviderUnavailable
    }
}

#[cfg(test)]
mod tests;
