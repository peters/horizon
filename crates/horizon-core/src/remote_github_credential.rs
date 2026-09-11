//! Explicit first-token delivery to an existing worker, never reconnect automation.

#[cfg(target_os = "linux")]
mod transport;

use crate::{
    cloud_run::{CloudWorkflowStore, StoredRemoteAllocation, interactive_worker::InteractiveWorkerProvider},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::RemotePanelStatusError,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Borrowed runtime-only secret. Syntax validation does not prove repository scope,
/// permission or expiry; those remain the user's authorization and GitHub's policy.
/// No serialization, persistence or implicit environment lookup is provided.
pub struct RepositoryPat<'a>(&'a str);

impl<'a> RepositoryPat<'a> {
    /// Accept the worker's bounded ASCII token alphabet, without a trailing newline.
    /// # Errors
    /// Empty, oversized or non-token input is rejected without exposing its value.
    pub fn new(value: &'a str) -> Result<Self, RemoteCredentialDeliveryError> {
        if value.is_empty()
            || value.len() > 16_384
            || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(RemoteCredentialDeliveryError::InvalidToken);
        }
        Ok(Self(value))
    }
}

impl std::fmt::Debug for RepositoryPat<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RepositoryPat([redacted])")
    }
}

/// Point-in-time installer result, not GitHub authorization or repository readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCredentialInstallation {
    Installed,
    Present,
}

/// Explicitly authorize first-token installation on one currently owned worker.
/// The caller must admit the actual session, provider/cost policy and disclosure of
/// this repository-scoped PAT. Inventory access alone is not such authorization.
/// Fresh non-creating provider inspection and retained SSH trust precede stdin-only
/// delivery. No keys, allocations, tasks, descriptors or saved state are created.
/// Existing credentials are never replaced; reconnect never calls this implicitly.
///
/// Run off the render thread. Stdin admission is capped to 15 seconds or the
/// remaining worker lease, including spawn elapsed time. Spawn/reap may still block.
/// Admission is point-in-time, not an atomic Stop fence or continuous revocation.
/// Failure after transport starts can leave the token installed or a private pending
/// file; do not automatically retry, rotate, repair, restart or terminate anything.
/// # Errors
/// Refuses unsupported clients, stale ownership, pending management, missing retained
/// identity, changed pins, non-ready workers and unavailable or uncertain transport.
pub fn install_remote_github_credential<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    token: &RepositoryPat<'_>,
) -> Result<RemoteCredentialInstallation, RemoteCredentialDeliveryError> {
    #[cfg(target_os = "linux")]
    {
        install_with(store, identities, provider, allocation, token, transport::install)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, provider, allocation, token.0);
        Err(RemoteCredentialDeliveryError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn install_with<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    token: &RepositoryPat<'_>,
    execute: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &RepositoryPat<'_>,
    ) -> Result<RemoteCredentialInstallation, RemoteCredentialDeliveryError>,
) -> Result<RemoteCredentialInstallation, RemoteCredentialDeliveryError> {
    use crate::remote_workspace_recovery::inspect_remote_allocation;
    let current = store
        .load_remote_allocation(
            allocation.workspace().session_id(),
            &allocation.workspace().state().spec.workspace_local_id,
        )
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    if current.as_ref() != Some(allocation) {
        return Err(RemoteWorkspaceRecoveryError::StateChanged.into());
    }
    allocation
        .recovery_request()
        .map_err(RemoteWorkspaceRecoveryError::from)?;
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(RemoteCredentialDeliveryError::MissingRetainedWorker)?;
    if runtime.worker.is_none()
        || !runtime
            .ssh
            .as_ref()
            .is_some_and(crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint::is_complete)
    {
        return Err(RemoteCredentialDeliveryError::MissingRetainedWorker);
    }
    let recovered = inspect_remote_allocation(identities, provider, allocation)?;
    validate_delivery(store, &recovered)?;
    let result = execute(store, &recovered, token)?;
    // Once delivery starts, stale state cannot establish that nothing was installed.
    validate_delivery(store, &recovered).map_err(|_| RemoteCredentialDeliveryError::DeliveryUnknown)?;
    Ok(result)
}

#[cfg(target_os = "linux")]
fn validate_delivery<'a>(
    store: &CloudWorkflowStore,
    recovered: &'a crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
) -> Result<&'a crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, RemoteCredentialDeliveryError> {
    let endpoint = crate::remote_worker_inspection::validate_current(store, recovered, None)?;
    if let Some(deadline) = lease_deadline(recovered)? {
        // Observation tolerates provider clock skew; secret release must not use
        // that tolerance to extend the worker's explicitly recorded deadline.
        if deadline <= time::OffsetDateTime::now_utc() {
            return Err(RemoteCredentialDeliveryError::ExpiredWorker);
        }
    }
    Ok(endpoint)
}

#[cfg(target_os = "linux")]
fn lease_deadline(
    recovered: &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
) -> Result<Option<time::OffsetDateTime>, RemoteCredentialDeliveryError> {
    recovered
        .observation()
        .and_then(|status| status.worker.lifetime.as_time_limited())
        .map(|lease| {
            time::OffsetDateTime::parse(&lease.terminate_after, &time::format_description::well_known::Rfc3339)
                .map_err(|_| RemoteCredentialDeliveryError::ExpiredWorker)
        })
        .transpose()
}

/// Diagnostics never contain the token, remote output, command or private paths.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteCredentialDeliveryError {
    #[error("repository token is empty, oversized or contains unsupported characters")]
    InvalidToken,
    #[error("credential delivery requires an existing worker and retained SSH host key")]
    MissingRetainedWorker,
    #[error("credential delivery requires an unexpired worker lifetime")]
    ExpiredWorker,
    #[error("credential delivery is unconfirmed; do not automatically retry or replace remote credentials")]
    DeliveryUnknown,
    #[error("protected credential delivery is not yet supported on this client platform")]
    UnsupportedPlatform,
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Admission(#[from] RemotePanelStatusError),
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(all(test, not(target_os = "linux")))]
mod platform_tests;
