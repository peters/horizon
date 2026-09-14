//! Explicit additions to saved Shell intent. No provider, key, Git or task I/O.

use std::path::PathBuf;

use super::{RemoteEnvironmentSummary, RemotePanelBinding, RemotePanelCommand, RemoteRuntimePhase};
use crate::{
    PanelKind,
    cloud_run::{CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, WorkerLifetime},
};

/// Caller-selected command and repository-relative directory for a new Shell task.
pub struct RemoteShellPanelDraft {
    pub command: RemotePanelCommand,
    pub working_directory: Option<String>,
}

/// A single-use local confirmation. Dropping it saves nothing and starts no task.
pub struct PreparedRemoteShellPanel {
    store_path: PathBuf,
    allocation: StoredRemoteAllocation,
    environment: RemoteEnvironmentSummary,
    panel: RemotePanelBinding,
}

impl PreparedRemoteShellPanel {
    #[must_use]
    pub fn environment(&self) -> &RemoteEnvironmentSummary {
        &self.environment
    }

    #[must_use]
    pub fn panel(&self) -> &RemotePanelBinding {
        &self.panel
    }
}

/// Newly saved intent and refreshed inventory; neither is proof of a running task.
pub struct AddedRemoteShellPanel {
    pub environment: RemoteEnvironmentSummary,
    pub panel_id: String,
}

/// Read an owned retained allocation and prepare one independent Shell intent.
/// Run off the render thread. The owning session must be the actual client session,
/// not a value copied from the selected inventory row. This reads only local saved
/// state: it neither certifies worker readiness nor grants execution authority.
/// # Errors
/// Rejects foreign/stale selections, missing retained trust, management conflicts,
/// invalid commands/directories and the workspace's existing panel limit.
pub fn prepare_remote_shell_panel(
    store: &CloudWorkflowStore,
    client_session_id: &str,
    expected: &RemoteEnvironmentSummary,
    draft: RemoteShellPanelDraft,
) -> Result<PreparedRemoteShellPanel, RemoteShellPanelError> {
    prepare_remote_shell_panel_with_id(store, client_session_id, expected, draft, uuid::Uuid::new_v4())
}

/// Re-admit a caller-journaled Shell intent with its original panel identity.
/// This never starts a task or restores an old workspace snapshot. The caller
/// must bind its journal to the owning workspace and retained allocation.
/// # Errors
/// Rejects nil/duplicate identities and the same stale, foreign, unavailable or
/// invalid state as [`prepare_remote_shell_panel`].
pub fn prepare_remote_shell_panel_with_id(
    store: &CloudWorkflowStore,
    client_session_id: &str,
    expected: &RemoteEnvironmentSummary,
    draft: RemoteShellPanelDraft,
    panel_id: uuid::Uuid,
) -> Result<PreparedRemoteShellPanel, RemoteShellPanelError> {
    if panel_id.is_nil() {
        return Err(RemoteShellPanelError::InvalidIntent);
    }
    let allocation = load_current(store, client_session_id, expected)?;
    let panel = RemotePanelBinding {
        panel_local_id: panel_id.to_string(),
        kind: PanelKind::Shell,
        command: Some(draft.command),
        working_directory: draft.working_directory,
        task_handoff: None,
        agent_session_id: None,
    };
    let mut next = allocation.workspace().state().clone();
    next.spec.panels.push(panel.clone());
    next.validate().map_err(|_| RemoteShellPanelError::InvalidIntent)?;
    Ok(PreparedRemoteShellPanel {
        store_path: store.path().into(),
        allocation,
        environment: expected.clone(),
        panel,
    })
}

/// Consume the displayed confirmation and append exactly its saved Shell intent.
/// Run off-thread. Recheck the actual session, selected revision and store at dispatch.
/// The existing snapshot CAS arbitrates competing additions and lifecycle changes;
/// a stale confirmation must be prepared again, never silently retried. Existing
/// panels, runtime, workflow, storage, public trust and checkpoints stay unchanged.
/// No remote or local task is started and no local view is created.
/// # Errors
/// Rejects context/state drift, management conflicts and storage failures. A failed
/// response does not authorize retrying the save without refreshing inventory.
pub fn add_remote_shell_panel(
    store: &CloudWorkflowStore,
    client_session_id: &str,
    expected: &RemoteEnvironmentSummary,
    prepared: PreparedRemoteShellPanel,
) -> Result<AddedRemoteShellPanel, RemoteShellPanelError> {
    if store.path() != prepared.store_path || *expected != prepared.environment {
        return Err(RemoteShellPanelError::StateChanged);
    }
    let current = load_current(store, client_session_id, expected)?;
    if current != prepared.allocation {
        return Err(RemoteShellPanelError::StateChanged);
    }
    let mut next = current.workspace().state().clone();
    let panel_id = prepared.panel.panel_local_id.clone();
    next.spec.panels.push(prepared.panel);
    let saved = store
        .replace_remote_workspace(current.workspace(), &next)
        .map_err(|error| storage_error(&error))?;
    Ok(AddedRemoteShellPanel {
        environment: saved.environment_summary(),
        panel_id,
    })
}

fn load_current(
    store: &CloudWorkflowStore,
    owner: &str,
    expected: &RemoteEnvironmentSummary,
) -> Result<StoredRemoteAllocation, RemoteShellPanelError> {
    if owner != expected.owning_session_id {
        return Err(RemoteShellPanelError::ClientSessionMismatch);
    }
    let allocation = store
        .load_remote_allocation(owner, &expected.workspace_local_id)
        .map_err(|error| storage_error(&error))?
        .ok_or(RemoteShellPanelError::StateChanged)?;
    if allocation.workspace().environment_summary() != *expected {
        return Err(RemoteShellPanelError::StateChanged);
    }
    let state = allocation.workspace().state();
    let runtime = state.runtime.as_ref().ok_or(RemoteShellPanelError::Unavailable)?;
    if state.spec.target.lifetime != WorkerLifetime::Persistent
        || runtime.cleanup.is_some()
        || !matches!(
            runtime.phase,
            RemoteRuntimePhase::Ready | RemoteRuntimePhase::Reconciling
        )
    {
        return Err(RemoteShellPanelError::Unavailable);
    }
    let request = allocation
        .worker_request()
        .map_err(|_| RemoteShellPanelError::Unavailable)?;
    let (worker, ssh) = runtime
        .worker
        .as_ref()
        .zip(runtime.ssh.as_ref())
        .ok_or(RemoteShellPanelError::Unavailable)?;
    if !worker.is_valid_for(request.target.provider)
        || worker.identity.workflow_id != request.workflow_id
        || worker.identity.job_id != request.job_id
        || worker.target != request.target
        || worker.ssh_public_key != request.ssh_public_key
        || !ssh.is_complete()
    {
        return Err(RemoteShellPanelError::Unavailable);
    }
    Ok(allocation)
}

fn storage_error(error: &RemoteWorkspaceStoreError) -> RemoteShellPanelError {
    match error {
        RemoteWorkspaceStoreError::SnapshotConflict | RemoteWorkspaceStoreError::RevisionConflict { .. } => {
            RemoteShellPanelError::StateChanged
        }
        _ => RemoteShellPanelError::StorageUnavailable,
    }
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteShellPanelError {
    #[error("the active client session does not own this saved environment")]
    ClientSessionMismatch,
    #[error("the saved environment changed; refresh before adding a Shell panel")]
    StateChanged,
    #[error("adding a Shell panel requires retained worker trust and no pending management operation")]
    Unavailable,
    #[error("the Shell command, directory or panel count is invalid")]
    InvalidIntent,
    #[error("the saved environment could not be safely read or updated")]
    StorageUnavailable,
}

#[cfg(test)]
mod tests;
