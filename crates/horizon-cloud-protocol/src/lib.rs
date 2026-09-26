//! Shared identity and placement types for host and worker coordination.
//! These records do not authorize provider calls or enable worker sharing.
#![forbid(unsafe_code)]
pub mod bootstrap;
pub mod companion;
mod identity;
pub mod inspection;
pub mod membership;
mod placement;
pub mod session_attachment;
pub mod session_runtime;
pub mod signed;

pub use identity::{AllocationId, ControllerId, OperationId, ProjectId, ProjectIdentity};
pub use placement::{ExistingWorker, Placement, PlacementBinding, SharingMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    #[error("Invalid cloud allocation identity")]
    Identity,
    #[error("Invalid cloud project membership")]
    Membership,
    #[error("Worker credential bindings require absolute local paths")]
    CredentialPath,
    #[error("Unsupported cloud placement version")]
    Version,
    #[error("Invalid cloud placement binding")]
    Encoding,
}

#[cfg(test)]
mod tests;
