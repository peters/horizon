//! Read-only fixed-root storage observation through an already retained SSH pin.

#[cfg(target_os = "linux")]
mod protocol;

use crate::{
    cloud_run::CloudWorkflowStore, remote_worker_status::RemotePanelStatusError,
    remote_workspace_recovery::RecoveredRemoteWorkspace,
    repository_overlay::intake::storage_status::WorkerStorageStatus,
};

/// Inspect the existing fixed worker root after owned, non-creating recovery.
/// Both complete allocation snapshots, management state, current lifetime and
/// retained worker/client/host identities are checked before and after the query.
/// The caller owns active-session selection and fresh provider/cost admission.
/// No root initialization, permission repair, setup, task, retry, cleanup or saved
/// state mutation occurs. A qualified observation grants no operation and proves
/// neither capacity, successful fsync, hardware health nor remote data retention.
/// Run off the render thread. The 15-second pipe deadline excludes spawn/reap;
/// local teardown does not stop the remote worker or independent tasks.
/// # Errors
/// Refuses unsupported client platforms, stale/unsafe ownership or identity,
/// unavailable SSH, incomplete input, timeout and malformed/inconsistent output.
/// A failed query is unknown, never an observed unsupported or unavailable root.
pub fn inspect_remote_worker_storage(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
) -> Result<WorkerStorageStatus, RemoteStorageInspectionError> {
    #[cfg(target_os = "linux")]
    {
        inspect_with(store, recovered, query)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (store, recovered);
        Err(RemoteStorageInspectionError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn inspect_with(
    store: &CloudWorkflowStore,
    recovered: &RecoveredRemoteWorkspace,
    execute: impl FnOnce(
        &crate::remote_ssh_identity::RemoteSshIdentity,
        &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
    ) -> Result<crate::remote_worker_ssh::query::Exchange, RemoteStorageInspectionError>,
) -> Result<WorkerStorageStatus, RemoteStorageInspectionError> {
    use crate::remote_worker_inspection::validate_current;
    let endpoint = validate_current(store, recovered, None)?;
    let response = execute(recovered.identity(), endpoint)?;
    let status = protocol::response(&response)?;
    validate_current(store, recovered, None)?;
    Ok(status)
}

#[cfg(target_os = "linux")]
fn query(
    identity: &crate::remote_ssh_identity::RemoteSshIdentity,
    endpoint: &crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint,
) -> Result<crate::remote_worker_ssh::query::Exchange, RemoteStorageInspectionError> {
    use crate::remote_worker_ssh::{known_hosts, prepared_storage_status};
    let trust = known_hosts(identity, endpoint)?;
    let command = prepared_storage_status(identity.private_key_path(), trust.path(), endpoint)?;
    exchange(command, std::time::Duration::from_secs(15))
}

#[cfg(target_os = "linux")]
fn exchange(
    command: std::process::Command,
    timeout: std::time::Duration,
) -> Result<crate::remote_worker_ssh::query::Exchange, RemoteStorageInspectionError> {
    use crate::remote_worker_ssh::query;
    // Valid negative observations exit 1: retain their output, unlike success-only queries.
    let mut input = protocol::REQUEST;
    query::exchange(command, &mut input, timeout, protocol::RESPONSE_LIMIT, || false, None).map_err(|error| match error
    {
        query::Error::ClientUnavailable => RemotePanelStatusError::ClientUnavailable.into(),
        query::Error::QueryFailed => RemoteStorageInspectionError::QueryFailed,
        query::Error::Deadline => RemoteStorageInspectionError::Deadline,
        query::Error::OutputLimit => RemoteStorageInspectionError::InvalidResponse,
    })
}

/// Diagnostics omit private paths, worker coordinates, keys and subprocess data.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteStorageInspectionError {
    #[error(transparent)]
    Admission(#[from] RemotePanelStatusError),
    #[error("remote storage query failed; no storage state was established")]
    QueryFailed,
    #[error("remote storage query exceeded its deadline; storage state is unknown")]
    Deadline,
    #[error("remote storage observation is invalid or exceeds its size limit")]
    InvalidResponse,
    #[error("protected remote storage inspection is not yet supported on this platform")]
    UnsupportedPlatform,
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
