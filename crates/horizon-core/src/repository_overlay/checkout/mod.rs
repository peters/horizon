//! Complete private raw checkouts, without publication, durability or task authority.

#[cfg(target_os = "linux")]
mod files;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod loose;

use super::{
    namespace::ResolvedRepositoryOverlay,
    seed::{GitObjectSource, SeedError},
};
use crate::cloud_run::ArtifactDigest;
use git2::Oid;
use std::{
    fmt,
    path::{Path, PathBuf},
};

/// Exact logical HEAD, independent index and raw working files in a private stage.
/// The caller must retain exclusive control. This is neither durable nor published;
/// it does not authorize export or task launch. Dropping it never deletes anything.
pub struct PreparedPrivateCheckout {
    path: PathBuf,
    base_commit: Oid,
    manifest_sha256: ArtifactDigest,
}

impl PreparedPrivateCheckout {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn base_commit(&self) -> Oid {
        self.base_commit
    }

    #[must_use]
    pub fn manifest_sha256(&self) -> &ArtifactDigest {
        &self.manifest_sha256
    }
}

impl fmt::Debug for PreparedPrivateCheckout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedPrivateCheckout")
            .finish_non_exhaustive()
    }
}

/// A redacted failure with an explicitly inspectable private residue, never auto-cleaned.
pub struct PrivateCheckoutFailure {
    pub reason: PrivateCheckoutError,
    residue: Option<PathBuf>,
}

impl PrivateCheckoutFailure {
    #[must_use]
    pub fn residue(&self) -> Option<&Path> {
        self.residue.as_deref()
    }
}

impl fmt::Debug for PrivateCheckoutFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateCheckoutFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for PrivateCheckoutFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl std::error::Error for PrivateCheckoutFailure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PrivateCheckoutError {
    #[error(transparent)]
    Seed(#[from] SeedError),
    #[error("private checkout storage failed")]
    Storage,
    #[error("private checkout node or identity is unsafe or changed")]
    UnsafeNode,
    #[error("private checkout object bytes are inconsistent")]
    Object,
    #[error("private checkout preparation was cancelled")]
    Cancelled,
}

/// Prepare a fresh seed and materialize the same immutable resolved working namespace.
/// No independently supplied seed can be paired with a different overlay. The caller
/// authorizes the complete base closure and guarantees stable ancestry/exclusive
/// same-user ownership of the private parent throughout seed creation and writing.
/// Linux confines working writes through pinned no-follow parents and creates links last;
/// libgit2's seed metadata pathname operations retain their documented trust premise.
/// Raw base bytes are incrementally decoded from this operation's own fresh loose store,
/// never through filters, hooks, packed-object fallback or whole-object mappings.
/// Run off the UI thread. Cancellation is checked between chunks/operations, but native
/// raw-file hashing and individual filesystem calls have no in-call cancellation bound.
/// No synchronization, publication, existing-checkout mutation or task launch occurs.
/// # Errors
/// Rejects unsupported platforms, unsafe nodes, inconsistent objects, I/O failures and
/// cancellation. Partial private stages remain available through the failure receipt.
pub fn prepare_private_checkout(
    parent: &Path,
    resolved: &ResolvedRepositoryOverlay,
    source: &mut impl GitObjectSource,
    cancelled: impl Fn() -> bool,
) -> Result<PreparedPrivateCheckout, PrivateCheckoutFailure> {
    #[cfg(target_os = "linux")]
    {
        linux::prepare(parent, resolved, source, &cancelled)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent, resolved, source, cancelled);
        Err(PrivateCheckoutFailure {
            reason: SeedError::Unsupported.into(),
            residue: None,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
