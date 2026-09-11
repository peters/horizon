//! Explicit ordinary Git setup on an existing pinned worker; never reconnect automation.

#[cfg(target_os = "linux")]
mod protocol;
#[cfg(target_os = "linux")]
mod transport;

use crate::{
    cloud_run::{CloudWorkflowStore, StoredRemoteAllocation, interactive_worker::InteractiveWorkerProvider},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_worker_status::RemotePanelStatusError,
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
};

/// Original preparation state, not current HEAD, cleanliness or task readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteGitState {
    Absent,
    ClaimedUnknown,
    Complete,
    Error,
}

/// Allowlisted worker diagnostics; remote text and repository paths are discarded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteGitReason {
    Invalid,
    Unsupported,
    UnsafeRoot,
    Conflict,
    Storage,
    Git,
    Interrupted,
    UnsupportedRepository,
}

/// A degraded completion (non-null reason) does not establish a usable checkout.
/// Absence is an observation, never permission to replay an uncertain submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteGitObservation {
    pub state: RemoteGitState,
    pub reason: Option<RemoteGitReason>,
}

/// Submission acknowledges only the detached handoff, not preparation completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteGitSubmission {
    Submitted,
    Observed(RemoteGitObservation),
    Unknown,
}

/// Explicitly admit one Git setup submission, using the exact saved repository and
/// runtime. `work_branch` must equal the saved, nonempty repository branch; callers
/// choose a dedicated branch at workspace setup, never invent one on reconnect.
/// This API cannot persist separate source/work branches or select a PR base.
///
/// Requires an existing worker and retained pin before noncreating inspection.
/// No provider creation, PAT installation, task start or local store writes occur.
/// The caller owns actual-session and cost authorization. Run off the render thread.
/// Stdin admission is capped to 60 seconds and the remaining worker lease, including
/// spawn elapsed time; spawn/reap may block. This is not atomic Stop revocation of
/// bytes already sent. Unknown outcomes must not cause automatic resubmission.
/// # Errors
/// Rejects invalid saved binding, missing trust, stale state, pending management,
/// expired workers and unavailable/unconfirmed SSH exchanges.
pub fn submit_remote_git_setup<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    work_branch: &str,
) -> Result<RemoteGitSubmission, RemoteGitSetupError> {
    #[cfg(target_os = "linux")]
    {
        operate_with(
            store,
            identities,
            provider,
            allocation,
            work_branch,
            |store, recovered, input| transport::execute(store, recovered, input, false),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, provider, allocation, work_branch);
        Err(RemoteGitSetupError::UnsupportedPlatform)
    }
}

/// Read the original Git receipt without launching preparation or granting replay.
/// Uses the same saved branch, ownership and pin admission as explicit submission.
/// Bounded to 40 seconds of stdin admission or the remaining lease; run off-thread.
/// # Errors
/// Refuses invalid or changed ownership/trust and unconfirmed observations.
pub fn inspect_remote_git_setup<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    work_branch: &str,
) -> Result<RemoteGitObservation, RemoteGitSetupError> {
    #[cfg(target_os = "linux")]
    {
        match operate_with(
            store,
            identities,
            provider,
            allocation,
            work_branch,
            |store, recovered, input| transport::execute(store, recovered, input, true),
        )? {
            RemoteGitSubmission::Observed(observation) => Ok(observation),
            _ => Err(RemoteGitSetupError::OutcomeUnknown),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, identities, provider, allocation, work_branch);
        Err(RemoteGitSetupError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn operate_with<P: InteractiveWorkerProvider + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    allocation: &StoredRemoteAllocation,
    work_branch: &str,
    execute: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &[u8],
    ) -> Result<RemoteGitSubmission, RemoteGitSetupError>,
) -> Result<RemoteGitSubmission, RemoteGitSetupError> {
    let input = protocol::request(allocation, work_branch)?;
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
        .ok_or(RemoteGitSetupError::MissingRetainedWorker)?;
    if runtime.worker.is_none()
        || !runtime
            .ssh
            .as_ref()
            .is_some_and(crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint::is_complete)
    {
        return Err(RemoteGitSetupError::MissingRetainedWorker);
    }
    let recovered = crate::remote_workspace_recovery::inspect_remote_allocation(identities, provider, allocation)?;
    validate_current(store, &recovered)?;
    let result = execute(store, &recovered, &input)?;
    validate_current(store, &recovered).map_err(|_| RemoteGitSetupError::OutcomeUnknown)?;
    Ok(result)
}

#[cfg(target_os = "linux")]
fn validate_current<'a>(
    store: &CloudWorkflowStore,
    recovered: &'a crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
) -> Result<&'a crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, RemoteGitSetupError> {
    let endpoint = crate::remote_worker_inspection::validate_current(store, recovered, None)?;
    if lease_deadline(recovered)?.is_some_and(|end| end <= time::OffsetDateTime::now_utc()) {
        return Err(RemoteGitSetupError::ExpiredWorker);
    }
    Ok(endpoint)
}

#[cfg(target_os = "linux")]
fn lease_deadline(
    recovered: &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
) -> Result<Option<time::OffsetDateTime>, RemoteGitSetupError> {
    recovered
        .observation()
        .and_then(|status| status.worker.lifetime.as_time_limited())
        .map(|lease| {
            time::OffsetDateTime::parse(&lease.terminate_after, &time::format_description::well_known::Rfc3339)
                .map_err(|_| RemoteGitSetupError::ExpiredWorker)
        })
        .transpose()
}

/// Static diagnostics never include SSH output, paths or request bytes.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteGitSetupError {
    #[error("Git setup requires the exact saved repository, runtime and explicit saved work branch")]
    InvalidBinding,
    #[error("Git setup requires an existing worker and retained SSH pin")]
    MissingRetainedWorker,
    #[error("Git setup requires an unexpired worker lifetime")]
    ExpiredWorker,
    #[error("Git setup outcome is unknown; do not automatically resubmit")]
    OutcomeUnknown,
    #[error("worker rejected the Git setup request")]
    Rejected,
    #[error("worker Git setup launcher is unavailable; no retry is implied")]
    Unavailable,
    #[error("remote Git setup is not yet supported on this client platform")]
    UnsupportedPlatform,
    #[error(transparent)]
    Recovery(#[from] RemoteWorkspaceRecoveryError),
    #[error(transparent)]
    Admission(#[from] RemotePanelStatusError),
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
