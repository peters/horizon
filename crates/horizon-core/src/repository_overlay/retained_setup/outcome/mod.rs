//! Historical execution receipts, never liveness, replay or cleanup authority.

#[cfg(any(target_os = "linux", test))]
pub(super) mod codec;
mod snapshot;

use super::{RetainedSetup, SetupBoundaryError, SetupClaimError, SetupExecutionError, SetupGrant, SetupIntent};
use crate::repository_overlay::materialize::MaterializedRepository;
pub use snapshot::{SetupCompletion, SetupCompletionState};

pub type SetupMaterializationResult = Result<MaterializedRepository, SetupExecutionError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SetupRecordError {
    #[error(transparent)]
    Boundary(#[from] SetupBoundaryError),
    #[error("retained setup result cannot be safely read")]
    Read,
    #[error("retained setup result is invalid, conflicting or exceeds its bound")]
    InvalidRecord,
    #[error("retained setup result already exists and cannot be replaced")]
    Existing,
    #[error("retained setup result synchronization is unconfirmed; retain all state")]
    Storage,
}

impl From<SetupClaimError> for SetupRecordError {
    fn from(error: SetupClaimError) -> Self {
        Self::Boundary(error.into())
    }
}

/// Recording uncertainty never discards an actual execution result or permits replay.
#[derive(Debug, thiserror::Error)]
#[error("{problem}")]
pub struct SetupRecordingFailure {
    pub problem: SetupRecordError,
    execution: Option<SetupMaterializationResult>,
}

impl SetupRecordingFailure {
    /// None means this invocation rejected recording preflight before execution.
    #[must_use]
    pub fn execution(&self) -> Option<&SetupMaterializationResult> {
        self.execution.as_ref()
    }

    #[must_use]
    pub fn into_execution(self) -> Option<SetupMaterializationResult> {
        self.execution
    }
}

impl RetainedSetup {
    /// Read only the matching claim's historical result; never inspect its paths.
    /// None means claimed but unknown, not unstarted, running or safe to replay.
    /// Observation does not acknowledge this reader's synchronization or liveness.
    /// Run off the UI thread; byte bounds do not bound filesystem I/O latency.
    /// # Errors
    /// Missing/conflicting claims and unsafe or malformed results are not absence.
    pub fn completion(&self, intent: &SetupIntent) -> Result<Option<SetupCompletion>, SetupRecordError> {
        #[cfg(target_os = "linux")]
        return self.directory.completion(intent);
        #[cfg(not(target_os = "linux"))]
        {
            let _ = intent;
            Err(SetupBoundaryError::Unsupported.into())
        }
    }
}

impl SetupGrant {
    /// Consume one grant, materialize and record its result in the retained root.
    /// An existing/unsafe result rejects execution; no record is adopted or replaced.
    /// The outer Ok acknowledges result-file and parent synchronization on qualified
    /// healthy storage; its inner result retains the materializer's exact outcome.
    /// Requires the same stable private ancestry/mount/source contracts as materialize.
    /// No supervisor, task launch, power-loss guarantee, replay or cleanup is provided.
    /// # Errors
    /// Record failures preserve an optional full execution result for inspection.
    /// A missing record after interruption remains unknown, even if a checkout exists.
    pub fn materialize_recorded(
        self,
        cancelled: impl Fn() -> bool,
    ) -> Result<SetupMaterializationResult, SetupRecordingFailure> {
        #[cfg(target_os = "linux")]
        {
            let directory = std::sync::Arc::clone(&self.directory);
            let intent = self.intent.clone();
            let preflight = directory
                .completion(&intent)
                .and_then(|existing| existing.map_or(Ok(()), |_| Err(SetupRecordError::Existing)));
            preflight.map_err(|problem| SetupRecordingFailure {
                problem,
                execution: None,
            })?;
            let execution = self.materialize(cancelled);
            let completion = SetupCompletion::from_execution(&execution);
            match directory.record(&intent, &completion) {
                Ok(()) => Ok(execution),
                Err(problem) => Err(SetupRecordingFailure {
                    problem,
                    execution: Some(execution),
                }),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cancelled;
            Err(SetupRecordingFailure {
                problem: SetupBoundaryError::Unsupported.into(),
                execution: None,
            })
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
