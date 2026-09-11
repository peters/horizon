//! Pinned panel inspection and an explicitly separate Git-bound Shell start API.
//! Calls are synchronous and must run off the render thread.

mod configured;
mod git_start;

pub use git_start::{RemoteGitTaskStartError, start_remote_git_shell};

pub use configured::{
    ConfiguredRemotePanelStatusError, ConfiguredRemotePanelStatusRequest, RemotePanelObservation,
    inspect_configured_remote_panel,
};

#[cfg(target_os = "linux")]
mod protocol;
#[cfg(target_os = "linux")]
mod ssh;

use crate::{cloud_run::CloudWorkflowStore, remote_workspace_recovery::RecoveredRemoteWorkspace};

/// A point-in-time task observation, never permission to start or replace a task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemotePanelStatus {
    Running { pid: u32 },
    Exited { pid: u32, exit_status: Option<u8> },
    Unavailable,
}

/// Inspect one saved panel after non-creating owned recovery. Both owned snapshots,
/// exact worker/host identity and current lifetime are checked before and after I/O.
/// The caller remains responsible for fresh provider/cost-policy admission. This
/// neither marks the workspace Ready nor authorizes attachment or task execution.
/// Dropping the local query never stops or deletes the worker or retained task.
/// # Errors
/// Rejects stale ownership, pending management, absent/non-ready workers, unknown
/// panels, unsupported key paths/platforms, failed SSH and invalid/bounded output.
pub fn inspect_remote_panel(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    panel_id: &str,
) -> Result<RemotePanelStatus, RemotePanelStatusError> {
    inspect(store, recovered, panel_id, Inspection::Status)
}

/// Compare the saved explicit shell/command intent with the retained task before
/// returning its status. No task, marker or session is created or replaced.
/// This does not certify repository contents, cost admission or attachment authority.
/// Calls are synchronous and must run off the render thread.
/// # Errors
/// Rejects the same conditions as [`inspect_remote_panel`], plus incomplete or
/// unsupported launch intent and a worker-side mismatch with its retained task.
pub fn inspect_remote_panel_intent(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    panel_id: &str,
) -> Result<RemotePanelStatus, RemotePanelStatusError> {
    inspect(store, recovered, panel_id, Inspection::SavedIntent)
}

#[derive(Clone, Copy)]
enum Inspection {
    Status,
    SavedIntent,
}

fn inspect(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    panel_id: &str,
    inspection: Inspection,
) -> Result<RemotePanelStatus, RemotePanelStatusError> {
    #[cfg(target_os = "linux")]
    {
        inspect_with(store, recovered, panel_id, inspection, ssh::request)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, recovered, panel_id, inspection);
        Err(RemotePanelStatusError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn inspect_with(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    panel_id: &str,
    inspection: Inspection,
    execute: impl FnOnce(
        &crate::remote_ssh_identity::RemoteSshIdentity,
        &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
        &[u8],
    ) -> Result<Vec<u8>, RemotePanelStatusError>,
) -> Result<RemotePanelStatus, RemotePanelStatusError> {
    let endpoint = validate_current(store, recovered, panel_id)?;
    let allocation = recovered.allocation();
    let runtime = allocation.worker_request()?.job_id;
    let request = match inspection {
        Inspection::Status => protocol::request(runtime, panel_id),
        Inspection::SavedIntent => protocol::intent_request(runtime, &allocation.workspace().state().spec, panel_id),
    }?;
    let response = execute(recovered.identity(), endpoint, &request)?;
    let status = protocol::response(&response, panel_id)?;
    validate_current(store, recovered, panel_id)?;
    Ok(status)
}

#[cfg(target_os = "linux")]
fn validate_current<'a>(
    store: &CloudWorkflowStore,
    recovered: &'a RecoveredRemoteWorkspace,
    panel_id: &str,
) -> Result<&'a crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint, RemotePanelStatusError> {
    crate::remote_worker_inspection::validate_current(store, recovered, Some(panel_id))
}

/// No error includes private paths, task data, provider payloads or subprocess output.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemotePanelStatusError {
    #[error("remote allocation changed; recover its current state before inspecting panels")]
    StateChanged,
    #[error("remote workspace has pending management intent")]
    ManagementPending,
    #[error("remote worker is not available through its retained pinned identity")]
    WorkerUnavailable,
    #[error("panel identity is not part of the owned remote workspace")]
    UnknownPanel,
    #[error("saved panel does not contain a fully supported explicit launch intent")]
    UnsupportedIntent,
    #[error("owned remote state could not be safely inspected")]
    StorageUnavailable,
    #[error("remote SSH identity or trust path is unsupported")]
    UnsupportedPath,
    #[error("private SSH trust material could not be prepared")]
    TrustStorage,
    #[error("SSH client is unavailable")]
    ClientUnavailable,
    #[error("remote panel query failed; no task was started or replaced")]
    QueryFailed,
    #[error("remote panel query exceeded its deadline")]
    Deadline,
    #[error("remote panel response is invalid or exceeds its size limit")]
    InvalidResponse,
    #[error("protected remote panel inspection is not yet supported on this platform")]
    UnsupportedPlatform,
}

impl From<crate::cloud_run::RemoteWorkspaceStoreError> for RemotePanelStatusError {
    fn from(error: crate::cloud_run::RemoteWorkspaceStoreError) -> Self {
        use crate::cloud_run::RemoteWorkspaceStoreError as Store;
        match error {
            Store::SnapshotConflict | Store::RevisionConflict { .. } => Self::StateChanged,
            Store::RuntimeRecoveryUnavailable => Self::ManagementPending,
            _ => Self::StorageUnavailable,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
