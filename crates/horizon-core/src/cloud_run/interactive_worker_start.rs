//! Opt-in compute start capability for a worker that was explicitly stopped, separate
//! from creation, attachment and the saved-task Start.

use super::interactive_worker::{InteractiveWorker, InteractiveWorkerProvider, InteractiveWorkerStatus};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerStart {
    /// Compute was started and the same exact worker was observed again through the
    /// provider's established readiness path. The status is `Ready` only once the
    /// endpoint is attested; until then it is `Provisioning`. This certifies nothing
    /// about in-memory work, which did not survive the stop.
    Started(InteractiveWorkerStatus),
    /// The exact worker was already running; nothing was restarted or re-posted.
    AlreadyRunning(InteractiveWorkerStatus),
    /// The exact resource was absent before any start command was attempted; nothing
    /// was allocated or replaced.
    AlreadyAbsent,
}

impl InteractiveWorkerStart {
    /// The observed status, when the worker exists.
    #[must_use]
    pub fn status(&self) -> Option<&InteractiveWorkerStatus> {
        match self {
            Self::Started(status) | Self::AlreadyRunning(status) => Some(status),
            Self::AlreadyAbsent => None,
        }
    }
}

/// An explicit, optional capability, the counterpart of the Stop capability. Callers
/// own durable authorization/intent fencing and must invoke this off the render thread.
/// Reconnecting, reopening a view or restarting the application never grants start
/// authority on its own.
pub trait InteractiveWorkerStartProvider: InteractiveWorkerProvider {
    /// Start compute for only the exact persisted worker, keeping its resource, storage
    /// and SSH identity. Implementations validate the handle before I/O, verify current
    /// ownership before mutation and again while waiting, never allocate or replace an
    /// absent resource, never restart a worker that is already running, and observe the
    /// same worker through their readiness path before answering. They must not delete,
    /// deliver credentials, prepare repositories or replay tasks.
    /// # Errors
    /// Rejects malformed/foreign handles and uncertain ownership, and reports an
    /// operation that did not reach a verified running state within its bound as
    /// unverified rather than guessing. Diagnostics must redact provider output.
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error>;
}

/// Provider coordinates are candidates, never an attestation or authority to connect.
/// The storage fingerprint compares two observations, not contents or durability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveWorkerEndpointCandidate {
    pub worker: InteractiveWorker,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub storage_fingerprint: super::ArtifactDigest,
    pub network_volume: Option<super::runpod::RunPodNetworkVolumeExpectation>,
}

/// Optional read-only coordinate discovery, separate from compute Start. Currently
/// implemented for retained `RunPod` workers; never calls initial host-key bootstrap.
pub trait InteractiveWorkerEndpointObserver: InteractiveWorkerProvider {
    /// Validate exact ownership, retained storage and the caller's original saved pin
    /// before returning a running worker's candidate coordinates. Each call must be
    /// bounded and GET-only. Absence or uncertain storage/readiness is an error.
    /// # Errors
    /// Rejects missing retained trust, foreign identity, absent/unready resources and
    /// unverifiable storage. The caller must still prove original-key possession.
    fn observe_endpoint_candidate(
        &self,
        worker: &InteractiveWorker,
        saved: &super::interactive_worker::InteractiveWorkerSshEndpoint,
    ) -> Result<InteractiveWorkerEndpointCandidate, Self::Error>;
}
