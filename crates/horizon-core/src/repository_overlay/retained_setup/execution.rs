use super::{SetupClaimError, SetupGrant};
use crate::repository_overlay::materialize::{MaterializationFailure, MaterializedRepository};

/// Consuming a grant never authorizes a retry, cleanup or a task start.
#[derive(Debug, thiserror::Error)]
pub enum SetupExecutionError {
    #[error(transparent)]
    Boundary(#[from] SetupBoundaryError),
    #[error(transparent)]
    Materialization(Box<MaterializationFailure>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SetupBoundaryError {
    #[error("retained setup execution was cancelled; existing state is retained")]
    Cancelled,
    #[error("retained setup execution requires supported storage and confinement")]
    Unsupported,
    #[error("retained setup root is missing, changed or not privately owned")]
    UnsafeRoot,
    #[error("retained setup claim cannot be safely read")]
    ClaimRead,
    #[error("retained setup claim is missing, invalid or conflicts with the admitted intent")]
    InvalidClaim,
    #[error("retained setup scratch slot already exists and cannot be reused")]
    ExistingScratch,
    #[error("retained setup storage operation failed; retain all state for inspection")]
    Storage,
}

impl From<SetupClaimError> for SetupBoundaryError {
    fn from(error: SetupClaimError) -> Self {
        match error {
            SetupClaimError::Unsupported => Self::Unsupported,
            SetupClaimError::UnsafeRoot => Self::UnsafeRoot,
            SetupClaimError::Read => Self::ClaimRead,
            SetupClaimError::InvalidIntent | SetupClaimError::InvalidRecord | SetupClaimError::Conflict => {
                Self::InvalidClaim
            }
            SetupClaimError::Storage => Self::Storage,
        }
    }
}

impl SetupGrant {
    /// Consume admission for one materialization into the fixed retained scratch slot.
    /// Intent and scratch cannot be replaced by the caller. Requires stable, trusted
    /// private ancestry, mounts and source throughout the existing materializer's I/O.
    /// Keep execution off the UI thread; cancellation cannot bound blocked filesystem
    /// latency. Drop retains the claim, scratch and all component residues.
    /// This synchronous receipt is not remote supervision or a stored terminal result.
    /// # Errors
    /// Boundary failures never run the materializer. Component failures preserve its
    /// typed retention/publication receipts. Lost results never authorize another grant.
    pub fn materialize(self, cancelled: impl Fn() -> bool) -> Result<MaterializedRepository, SetupExecutionError> {
        if cancelled() {
            return Err(SetupBoundaryError::Cancelled.into());
        }
        #[cfg(target_os = "linux")]
        {
            use crate::repository_overlay::materialize::{MaterializationRequest, materialize_repository};

            let scratch = self.directory.scratch(&self.intent)?;
            if cancelled() {
                return Err(SetupBoundaryError::Cancelled.into());
            }
            let request = MaterializationRequest {
                objects_directory: &self.intent.objects_directory,
                bundle_store: &self.intent.bundle_store,
                bundle_manifest: &self.intent.bundle_manifest,
                scratch_parent: scratch.path(),
                destination: &self.intent.destination,
            };
            materialize_repository(&request, cancelled)
                .map_err(|error| SetupExecutionError::Materialization(Box::new(error)))
        }
        #[cfg(not(target_os = "linux"))]
        Err(SetupBoundaryError::Unsupported.into())
    }
}

#[cfg(test)]
mod tests;
