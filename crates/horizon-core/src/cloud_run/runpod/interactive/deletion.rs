//! Point-in-time, exact-Pod deletion observation without guest access.

use super::super::{
    RunPodClient, RunPodError, RunPodNetworkVolumeExpectation, RunPodProfile, status_from_resource, validate_target,
};
use super::{NetworkBinding, RunPodInteractiveWorkerProvider, RunPodSshEndpoint, RunPodWorker, runpod_worker};
use crate::cloud_run::{
    CloudProvider,
    interactive_worker::{
        InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerProvider,
        InteractiveWorkerRequest, InteractiveWorkerStatus,
    },
    interactive_worker_delete::{InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation},
};

/// Exact network-backed Pod cleanup without an SSH host pin or guest access.
///
/// The caller admits the saved allocation, account/profile and storage binding
/// before constructing the client. This wrapper does not expose Start or Stop;
/// provisioning, recovery and generic readiness inspection return `InvalidTarget`
/// without I/O. Deletion retains the existing volume and Pod ownership checks.
pub struct RunPodDeletionWorkerProvider {
    inner: RunPodInteractiveWorkerProvider,
}

impl RunPodDeletionWorkerProvider {
    /// Bind a caller-admitted persistent request and network-volume selection.
    ///
    /// No host-key source can be supplied, and construction performs no I/O.
    /// This does not establish volume ownership, retention or deletion authority.
    /// # Errors
    /// Rejects malformed or conflicting request/profile/selection bindings.
    pub fn new_with_network_volume(
        mut client: RunPodClient,
        profile: RunPodProfile,
        expected: &InteractiveWorkerRequest,
        selection: &RunPodNetworkVolumeExpectation,
    ) -> Result<Self, RunPodError> {
        let binding = NetworkBinding::new(expected, selection, &profile)?;
        client.bind_network(&binding)?;
        Ok(Self {
            inner: RunPodInteractiveWorkerProvider::new(client, profile, unavailable_host_key),
        })
    }
}

fn unavailable_host_key(_: &RunPodWorker, _: &RunPodSshEndpoint, _: &str) -> Option<String> {
    None
}

impl InteractiveWorkerProvider for RunPodDeletionWorkerProvider {
    type Error = RunPodError;

    fn provider(&self) -> CloudProvider {
        CloudProvider::RunPod
    }

    fn ensure_worker(&self, _request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        Err(RunPodError::InvalidTarget)
    }

    fn reconcile_worker(
        &self,
        _request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(RunPodError::InvalidTarget)
    }

    fn inspect_worker(&self, _worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Err(RunPodError::InvalidTarget)
    }

    fn delete_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.inner.delete_worker(worker)
    }
}

impl InteractiveWorkerDeleteObserver for RunPodDeletionWorkerProvider {
    fn observe_worker_deletion(
        &self,
        worker: &InteractiveWorker,
    ) -> Result<InteractiveWorkerDeletionObservation, Self::Error> {
        self.inner.observe_worker_deletion(worker)
    }
}

impl InteractiveWorkerDeleteObserver for RunPodInteractiveWorkerProvider {
    fn observe_worker_deletion(
        &self,
        worker: &InteractiveWorker,
    ) -> Result<InteractiveWorkerDeletionObservation, RunPodError> {
        self.client.check_network_worker(worker)?;
        let retained = runpod_worker(worker)?;
        validate_target(&worker.target, &self.profile)?;
        let Some(pod) = self.client.transport.get(&retained.pod_id)? else {
            return Ok(InteractiveWorkerDeletionObservation::Absent);
        };
        status_from_resource(&pod, &retained, Some(&worker.ssh_public_key))?;
        self.client.verify_attachment(&pod)?;
        // A terminated Pod record still exists. This observes only the Pod's
        // deletion scope, not independent storage, readiness or retained bytes.
        Ok(InteractiveWorkerDeletionObservation::Present)
    }
}
