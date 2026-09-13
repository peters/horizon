//! Explicit, durable Start of a stopped worker's retained compute. Reconnecting,
//! reopening a view or restarting the application never invokes this operation.

mod configured_azure;
pub(crate) mod endpoint;

pub use configured_azure::{ConfiguredAzureStart, ConfiguredAzureStartError, start_configured_azure_environment};
pub use endpoint::{RemoteEndpointRefreshError, refresh_remote_worker_endpoint};

use crate::{
    cloud_run::{
        CloudStoreError, CloudWorkflowStore, RemoteWorkspaceStoreError, StoredRemoteAllocation, WorkerLifetime,
        interactive_worker::InteractiveWorkerLifecycle,
        interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    },
    remote_workspace::RemoteRuntimePhase,
};

/// The record after a verified start: the same worker under the same saved identity,
/// now `Reconciling`. Reconnecting session panels re-establishes readiness; nothing
/// here resumed a task, and in-memory work did not survive the stop.
#[derive(Debug, Eq, PartialEq)]
pub struct RemoteWorkspaceStart {
    pub allocation: StoredRemoteAllocation,
    /// The provider's lifecycle after the start (`Ready` only with an attested endpoint).
    pub lifecycle: InteractiveWorkerLifecycle,
    /// The exact worker was already running; nothing was started or re-posted.
    pub already_running: bool,
}

/// Record explicit Start intent over a saved Stopped record, start only the retained
/// worker's compute, then record the renewed observation. Requires the exact current
/// allocation snapshot with a retained persistent worker and a complete saved pin.
/// The provider must never allocate or replace an absent worker; the observed worker and
/// any observed endpoint must equal the saved ones, so a replacement pin is never adopted.
/// Provider failure, absence and identity drift retain the intent and identity for an
/// explicit retry, which reuses the original request time and re-posts nothing that is
/// already running. Stop and recovery refuse a record with Start intent until it resolves.
/// Run off the render thread, only after explicit confirmation.
/// # Errors
/// Rejects stale ownership, records without a saved Stop, missing worker or pin,
/// provider mismatch, competing management intent, unverified starts, absence, identity
/// drift, unsafe clocks and storage failures. Errors redact provider payloads.
pub fn start_remote_workspace<P: InteractiveWorkerStartProvider + ?Sized>(
    store: &CloudWorkflowStore,
    provider: &P,
    expected: &StoredRemoteAllocation,
) -> Result<RemoteWorkspaceStart, RemoteWorkspaceStartError> {
    use RemoteWorkspaceStartError as Error;
    let allocation = store
        .load_remote_allocation(
            expected.workspace().session_id(),
            &expected.workspace().state().spec.workspace_local_id,
        )?
        .ok_or(Error::MissingAllocation)?;
    if allocation != *expected {
        return Err(Error::StateChanged);
    }
    let runtime = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .ok_or(Error::MissingAllocation)?;
    let worker = runtime.worker.as_ref().ok_or(Error::MissingWorker)?;
    if !worker.is_valid_for(provider.provider()) {
        return Err(Error::ProviderMismatch);
    }
    if worker.target.lifetime != WorkerLifetime::Persistent {
        return Err(Error::UnsupportedLifetime);
    }
    if runtime.cleanup.is_some() {
        return Err(Error::ManagementConflict);
    }
    let ssh = runtime
        .ssh
        .as_ref()
        .filter(|ssh| ssh.is_complete())
        .ok_or(Error::MissingTrust)?;
    let now_millis = current_millis()?;
    let requested_at_millis = match runtime.phase {
        RemoteRuntimePhase::Stopped { .. } => now_millis,
        RemoteRuntimePhase::Starting { requested_at_millis } => requested_at_millis,
        _ => return Err(Error::NotStopped),
    };
    if requested_at_millis > now_millis {
        return Err(Error::InvalidTimestamp);
    }
    let starting =
        store.record_remote_start_phase(&allocation, RemoteRuntimePhase::Starting { requested_at_millis })?;
    let (status, already_running) = match provider.start_worker(worker).map_err(|_| Error::ProviderUnavailable)? {
        InteractiveWorkerStart::AlreadyAbsent => return Err(Error::ResourceAbsent),
        InteractiveWorkerStart::Started(status) => (status, false),
        InteractiveWorkerStart::AlreadyRunning(status) => (status, true),
    };
    // Only the exact saved worker under its saved trust counts as started: another
    // identity, or an endpoint that differs from the saved pin, is never adopted.
    if status.worker != *worker || status.ssh.as_ref().is_some_and(|observed| observed != ssh) {
        return Err(Error::IdentityMismatch);
    }
    let allocation = store.record_remote_start_phase(&starting, RemoteRuntimePhase::Reconciling)?;
    Ok(RemoteWorkspaceStart {
        allocation,
        lifecycle: status.lifecycle,
        already_running,
    })
}

fn current_millis() -> Result<i64, RemoteWorkspaceStartError> {
    i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .ok()
        .filter(|millis| *millis >= 0)
        .ok_or(RemoteWorkspaceStartError::InvalidTimestamp)
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteWorkspaceStartError {
    #[error("no current owned allocation is available to start")]
    MissingAllocation,
    #[error("saved environment has no exact retained worker to start")]
    MissingWorker,
    #[error("Start requires an already retained complete SSH pin")]
    MissingTrust,
    #[error("only a saved Stopped environment, or one with Start intent, can be started")]
    NotStopped,
    #[error("saved environment changed; refresh before explicitly retrying Start")]
    StateChanged,
    #[error("Start provider does not match the saved worker")]
    ProviderMismatch,
    #[error("explicit Start requires a persistent execution policy")]
    UnsupportedLifetime,
    #[error("existing management intent does not permit this Start request")]
    ManagementConflict,
    #[error("worker Start could not be verified; saved intent and identity remain retained")]
    ProviderUnavailable,
    #[error("worker is absent; nothing was allocated or replaced, and saved intent remains")]
    ResourceAbsent,
    #[error("the started worker or its endpoint does not match the saved identity; saved intent remains")]
    IdentityMismatch,
    #[error("Start timestamp could not be safely recorded")]
    InvalidTimestamp,
    #[error("owned remote Start state could not be safely accessed")]
    StorageUnavailable,
}

impl From<RemoteWorkspaceStoreError> for RemoteWorkspaceStartError {
    fn from(error: RemoteWorkspaceStoreError) -> Self {
        match error {
            RemoteWorkspaceStoreError::RevisionConflict { .. } | RemoteWorkspaceStoreError::SnapshotConflict => {
                Self::StateChanged
            }
            _ => Self::StorageUnavailable,
        }
    }
}

impl From<CloudStoreError> for RemoteWorkspaceStartError {
    fn from(_: CloudStoreError) -> Self {
        Self::StorageUnavailable
    }
}

impl From<rusqlite::Error> for RemoteWorkspaceStartError {
    fn from(_: rusqlite::Error) -> Self {
        Self::StorageUnavailable
    }
}

#[cfg(test)]
mod tests;
