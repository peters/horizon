//! Opt-in compute Stop capability, separate from resource deletion and attachment.

use super::interactive_worker::{InteractiveWorker, InteractiveWorkerProvider};

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
