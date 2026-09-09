//! Shared current-owned-worker fence for read-only queries, never execution authority.

use crate::{
    cloud_run::{CloudWorkflowStore, interactive_worker::InteractiveWorkerSshEndpoint},
    remote_worker_status::RemotePanelStatusError as Error,
    remote_workspace_recovery::RecoveredRemoteWorkspace,
};

/// A workspace query has no panel scope; a panel query additionally checks its
/// genuine saved identity. Preserve the admission order for existing panel calls.
pub(crate) fn validate_current<'a>(
    store: &CloudWorkflowStore,
    recovered: &'a RecoveredRemoteWorkspace,
    panel_id: Option<&str>,
) -> Result<&'a InteractiveWorkerSshEndpoint, Error> {
    let allocation = recovered.allocation();
    let workspace = allocation.workspace();
    let current = store.load_remote_allocation(workspace.session_id(), &workspace.state().spec.workspace_local_id)?;
    if current.as_ref() != Some(allocation) {
        return Err(Error::StateChanged);
    }
    let request = allocation.recovery_request()?;
    if panel_id.is_some_and(|id| {
        !workspace
            .state()
            .spec
            .panels
            .iter()
            .any(|panel| panel.panel_local_id == id)
    }) {
        return Err(Error::UnknownPanel);
    }
    let runtime = workspace.state().runtime.as_ref().ok_or(Error::WorkerUnavailable)?;
    let observation = recovered.observation().ok_or(Error::WorkerUnavailable)?;
    if !observation.is_ready_for(&request, time::OffsetDateTime::now_utc())
        || recovered.identity().public_key() != request.ssh_public_key
        || runtime.worker.as_ref() != Some(&observation.worker)
        || runtime.ssh != observation.ssh
    {
        return Err(Error::WorkerUnavailable);
    }
    observation.ssh.as_ref().ok_or(Error::WorkerUnavailable)
}
