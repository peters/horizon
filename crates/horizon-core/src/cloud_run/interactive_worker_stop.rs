//! Opt-in compute Stop capability, separate from resource deletion and attachment.

use super::interactive_worker::{InteractiveWorker, InteractiveWorkerProvider};
use super::{interactive_worker::InteractiveWorkerSshEndpoint, runpod::RunPodNetworkVolumeExpectation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerStop {
    /// The exact resource is retained and verified inactive, including an idempotent retry.
    /// This does not certify successful task exit, preserved process memory or a checkpoint.
    Stopped,
    /// The exact resource was already absent before any stop command was attempted.
    AlreadyAbsent,
}

/// An explicit, optional capability; unsupported providers must never substitute deletion.
/// Callers own durable authorization/intent fencing and must invoke this off the render
/// thread. Closing a client, view, session or application never grants Stop authority.
pub trait InteractiveWorkerStopProvider: InteractiveWorkerProvider {
    /// Stop only the exact persisted worker, retaining its resource and storage.
    /// Implementations validate the handle before I/O, verify current ownership before
    /// mutation, reject implicit resource deletion policies, and verify retained inactive
    /// state afterward. They must not create, restart, delete, remove storage, run SSH,
    /// or infer task success. A caller must separately protect/checkpoint in-memory work.
    /// # Errors
    /// Rejects malformed/foreign handles, uncertain ownership or data retention, failed
    /// provider operations and unverified completion. Diagnostics must redact provider output.
    fn stop_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStop, Self::Error>;
}

/// Saved public identity and storage selection, not new Stop or trust authority.
#[derive(Clone, Copy)]
pub struct InteractiveWorkerStopExpectation<'a> {
    pub worker: &'a InteractiveWorker,
    /// Preserved and shape-checked only; observation does not verify an SSH handshake.
    pub ssh: &'a InteractiveWorkerSshEndpoint,
    pub network_volume: Option<&'a RunPodNetworkVolumeExpectation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerStopObservation {
    /// Exact resource and selected storage remain present, with verified inactive compute.
    /// Not a checkpoint, task result, live host-pin check or filesystem durability proof.
    RetainedStopped,
    /// The exact retained resource is not yet verified stopped. No mutation was attempted.
    Pending,
    /// The exact resource was absent. This must never be promoted to retained Stop success.
    Absent,
}

/// Optional provider-read-only capability, deliberately separate from issuing Stop.
pub trait InteractiveWorkerStopObserver: InteractiveWorkerProvider {
    /// Observe one exact saved worker and storage binding off the render thread.
    /// Validate the complete expectation before I/O, then verify retained ownership,
    /// inactive compute and storage. Never Stop, create, restart, delete, reconcile,
    /// use SSH, obtain host keys, or turn absence/termination into retained success.
    /// # Errors
    /// Rejects invalid or mismatched identity/storage, uncertain metadata and failed
    /// observations. Diagnostics must not include provider response payloads.
    fn observe_worker_stop(
        &self,
        expected: InteractiveWorkerStopExpectation<'_>,
    ) -> Result<InteractiveWorkerStopObservation, Self::Error>;
}
