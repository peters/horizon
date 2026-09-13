//! Explicit authenticated coordinate refresh, never compute or task Start.

use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::{InteractiveWorker, InteractiveWorkerSshEndpoint},
        interactive_worker_start::{InteractiveWorkerEndpointCandidate, InteractiveWorkerEndpointObserver},
        runpod::RunPodNetworkVolumeExpectation,
    },
    remote_ssh_identity::{RemoteSshIdentity, RemoteSshIdentityStore},
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
};

/// Refresh only the host/port of one exact retained `RunPod` connection after explicit
/// user action. Uses the existing private identity and original host key to execute
/// only `/usr/bin/true`, then independently re-observes ownership/storage/coordinates
/// before an exact-snapshot CAS. No initial trust, new keys, provider mutation,
/// repository credentials, task replay or phase transition occurs. Start intent and
/// its original timestamp remain; Start/recovery are separate explicit operations.
/// Run off the render thread. Provider reads use their existing request bounds; SSH
/// pipe I/O is bounded to ten seconds (spawn/reap retain the query helper's OS limits).
/// # Errors
/// Refuses unsupported platforms/providers, absent identity, management conflicts,
/// stale snapshots, changed storage, failed authentication and failed reads.
pub fn refresh_remote_worker_endpoint<P: InteractiveWorkerEndpointObserver + ?Sized>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, RemoteEndpointRefreshError> {
    if !cfg!(target_os = "linux") {
        return Err(Error::Unsupported);
    }
    refresh_with(
        store,
        provider,
        expected,
        |worker| {
            identities
                .recover(
                    worker.identity.workflow_id,
                    worker.identity.job_id,
                    &worker.ssh_public_key,
                )
                .map_err(|_| Error::IdentityUnavailable)
        },
        prove_connection,
    )
}

fn prove_connection(identity: &RemoteSshIdentity, endpoint: &InteractiveWorkerSshEndpoint) -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        crate::remote_worker_ssh::prove_endpoint(identity, endpoint).map_err(|_| Error::AuthenticationFailed)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (identity, endpoint);
        Err(Error::Unsupported)
    }
}

pub(crate) fn refresh_phase_allowed(state: &RemoteWorkspaceState) -> bool {
    state.runtime.as_ref().is_some_and(|runtime| {
        runtime.cleanup.is_none()
            && matches!(
                runtime.phase,
                RemoteRuntimePhase::Ready
                    | RemoteRuntimePhase::Reconciling
                    | RemoteRuntimePhase::Stopped { .. }
                    | RemoteRuntimePhase::Starting { .. }
            )
    })
}

fn current(store: &CloudWorkflowStore, expected: &StoredRemoteAllocation) -> Result<(), Error> {
    let actual = store
        .load_remote_allocation(
            expected.workspace().session_id(),
            &expected.workspace().state().spec.workspace_local_id,
        )
        .map_err(|_| Error::StorageUnavailable)?;
    if actual.as_ref() != Some(expected) {
        return Err(Error::StateChanged);
    }
    Ok(())
}

fn selection(
    store: &CloudWorkflowStore,
    expected: &StoredRemoteAllocation,
) -> Result<Option<RunPodNetworkVolumeExpectation>, Error> {
    store
        .load_remote_network_volume_selection(expected)
        .map_err(|_| Error::StorageUnavailable)
}

fn refresh_with<P: InteractiveWorkerEndpointObserver + ?Sized, I>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
    recover: impl FnOnce(&InteractiveWorker) -> Result<I, Error>,
    probe: impl FnOnce(&I, &InteractiveWorkerSshEndpoint) -> Result<(), Error>,
) -> Result<StoredRemoteAllocation, Error> {
    current(store, expected)?;
    let state = expected.workspace().state();
    if !refresh_phase_allowed(state) {
        return Err(Error::ManagementConflict);
    }
    let runtime = state.runtime.as_ref().ok_or(Error::IdentityUnavailable)?;
    let worker = runtime.worker.as_ref().ok_or(Error::IdentityUnavailable)?;
    let saved = runtime
        .ssh
        .as_ref()
        .filter(|ssh| ssh.is_complete())
        .ok_or(Error::IdentityUnavailable)?;
    if provider.provider() != CloudProvider::RunPod
        || !worker.is_valid_for(CloudProvider::RunPod)
        || worker.target.lifetime != WorkerLifetime::Persistent
    {
        return Err(Error::Unsupported);
    }
    let volume = selection(store, expected)?;
    let identity = recover(worker)?;
    current(store, expected)?;
    let before = provider
        .observe_endpoint_candidate(worker, saved)
        .map_err(|_| Error::ObservationFailed)?;
    let candidate = bind_candidate(&before, worker, saved, volume.as_ref())?;
    current(store, expected)?;
    probe(&identity, &candidate)?;
    current(store, expected)?;
    let after = provider
        .observe_endpoint_candidate(worker, saved)
        .map_err(|_| Error::ObservationFailed)?;
    if before != after || selection(store, expected)? != volume {
        return Err(Error::ObservationChanged);
    }
    current(store, expected)?;
    store.record_remote_endpoint_refresh(expected, &candidate)
}

fn bind_candidate(
    candidate: &InteractiveWorkerEndpointCandidate,
    worker: &InteractiveWorker,
    saved: &InteractiveWorkerSshEndpoint,
    volume: Option<&RunPodNetworkVolumeExpectation>,
) -> Result<InteractiveWorkerSshEndpoint, Error> {
    if candidate.worker != *worker
        || candidate.username != saved.username
        || candidate.network_volume.as_ref() != volume
    {
        return Err(Error::ObservationChanged);
    }
    let mut endpoint = saved.clone();
    endpoint.host.clone_from(&candidate.host);
    endpoint.port = candidate.port;
    if !endpoint.is_complete() {
        return Err(Error::ObservationChanged);
    }
    Ok(endpoint)
}

type Error = RemoteEndpointRefreshError;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteEndpointRefreshError {
    #[error("connection refresh requires a retained persistent RunPod worker on Linux")]
    Unsupported,
    #[error("existing SSH identity or retained public pin is unavailable; no replacement was generated")]
    IdentityUnavailable,
    #[error("saved environment changed; refresh the selection before proceeding")]
    StateChanged,
    #[error("active management or cleanup prevents connection refresh")]
    ManagementConflict,
    #[error("retained worker coordinates or storage could not be observed")]
    ObservationFailed,
    #[error("observed worker identity, storage or coordinates changed")]
    ObservationChanged,
    #[error("original SSH identity could not be authenticated at the observed coordinates")]
    AuthenticationFailed,
    #[error("saved connection could not be safely accessed")]
    StorageUnavailable,
}

#[cfg(test)]
mod tests;
