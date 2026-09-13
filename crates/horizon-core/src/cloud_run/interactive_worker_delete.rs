//! Opt-in, control-plane-only observation of one exact worker's deletion scope.

use super::interactive_worker::{InteractiveWorker, InteractiveWorkerProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractiveWorkerDeletionObservation {
    /// The exact owned deletion scope still exists, including deletion in progress.
    /// This does not describe compute readiness, retained bytes or task state.
    Present,
    /// The exact deletion scope was absent at this observation. A missing child
    /// resource alone is insufficient, and this result does not authorize replay.
    Absent,
}

/// Optional read-only capability, separate from requesting resource deletion.
/// Callers own durable intent, configured admission and completion recording.
pub trait InteractiveWorkerDeleteObserver: InteractiveWorkerProvider {
    /// Observe the complete scope of the exact persisted worker off the render thread.
    /// Validate the handle before I/O and current ownership before reporting presence.
    /// Use control-plane reads only: never create, delete, start, stop, reconcile,
    /// execute guest commands, use SSH, obtain host keys or retry a mutation.
    /// A surviving deletion scope remains present even if its compute is gone.
    ///
    /// # Errors
    /// Rejects malformed or mismatched identity, uncertain metadata and failed reads.
    /// Errors and accepted deletion responses must never be translated into absence;
    /// diagnostics must not include provider response payloads.
    fn observe_worker_deletion(
        &self,
        worker: &InteractiveWorker,
    ) -> Result<InteractiveWorkerDeletionObservation, Self::Error>;
}
