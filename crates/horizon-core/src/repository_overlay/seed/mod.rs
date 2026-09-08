//! Unpublished exact-base Git material; never a ready checkout or export authority.

#[cfg(target_os = "linux")]
pub mod export;
#[cfg(target_os = "linux")]
mod import;
#[cfg(target_os = "linux")]
mod index;
#[cfg(target_os = "linux")]
pub mod packed;
#[cfg(target_os = "linux")]
mod staging;

use super::namespace::ResolvedRepositoryOverlay;
#[cfg(target_os = "linux")]
pub(super) const MAX_OBJECTS: usize = super::MAX_CHANGES * 2 + 2;
#[cfg(target_os = "linux")]
pub(super) const MAX_BYTES: u64 =
    super::MAX_CONTENT_BYTES + super::bundle::MAX_BUNDLE_BYTES as u64 + 3 * super::MAX_METADATA_BYTES as u64;
use git2::{ObjectType, Oid};
use std::{
    fmt,
    io::Read,
    path::{Path, PathBuf},
};

/// One raw Git object stream, without Git's type/length hash prefix.
/// The importer checks kind, exact length and the destination-computed object ID.
pub struct GitObjectStream<'a> {
    pub kind: ObjectType,
    pub bytes: u64,
    pub reader: Box<dyn Read + 'a>,
}

/// Explicitly authorized source of raw objects, independent of their storage format.
/// Implementations own their memory, blocking, cancellation and transport policy.
/// The importer does not provide an unbounded buffered or loose-only Git adapter.
pub trait GitObjectSource {
    /// # Errors
    /// Return a redacted failure when the requested object cannot be streamed.
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError>;
}

/// Header claims only; consumers must verify payload identity when reading contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitObjectMetadata {
    pub kind: ObjectType,
    pub bytes: u64,
}

/// A source that can inspect objects without opening or abandoning payload streams.
/// Inspection follows the source's existing resource, cancellation and trust policy.
pub trait GitObjectInspector: GitObjectSource {
    /// # Errors
    /// Return a redacted failure without silently restarting a failed source session.
    fn inspect(&mut self, object: Oid) -> Result<GitObjectMetadata, SeedError>;
}

/// Logically verified private Git seed. No working files, publication or durability proof.
/// The path and its ancestry must remain exclusively controlled until later preparation
/// and publication. Dropping this receipt never deletes data.
pub struct PreparedGitSeed {
    path: PathBuf,
    base_commit: Oid,
    objects: usize,
}

impl PreparedGitSeed {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn base_commit(&self) -> Oid {
        self.base_commit
    }

    #[must_use]
    pub fn imported_objects(&self) -> usize {
        self.objects
    }
}

impl fmt::Debug for PreparedGitSeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedGitSeed")
            .field("objects", &self.objects)
            .finish_non_exhaustive()
    }
}

/// Failure with an optional private residue. No cleanup is implicit; the caller must
/// prove ownership before removing a residue. Display and Debug redact the path.
pub struct SeedFailure {
    pub reason: SeedError,
    residue: Option<PathBuf>,
}

impl SeedFailure {
    #[must_use]
    pub fn residue(&self) -> Option<&Path> {
        self.residue.as_deref()
    }
}

impl fmt::Debug for SeedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeedFailure")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for SeedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl std::error::Error for SeedFailure {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SeedError {
    #[error("private Git seed preparation is unsupported on this platform")]
    Unsupported,
    #[error("Git seed preparation requires a trusted private directory")]
    UnsafeParent,
    #[error("private Git seed storage failed")]
    Storage,
    #[error("repository object source failed")]
    Source,
    #[error("repository object identity, type or length is inconsistent")]
    Object,
    #[error("Git seed preparation exceeds a supported bound")]
    Limit,
    #[error("Git seed preparation was cancelled")]
    Cancelled,
}

/// Create a fresh private repository with exact detached HEAD, shallow boundary and
/// independently constructed stage-zero index. No working files or tasks are created.
/// The caller authorizes the complete base closure, including removed files and exact
/// commit metadata, and guarantees stable ancestry/exclusive ownership of `parent`.
/// Linux checks private ownership and rejects symlink ancestry; libgit2 pathname writes
/// are not confined against concurrent same-user/privileged mutation. No hooks, filters,
/// repository configuration, parent history, refs or alternates are imported.
/// Run off the UI thread. Cancellation is checked between source operations/chunks;
/// a blocking source call and native Git/storage work need their own latency controls.
/// # Errors
/// Rejects unsafe parents, inconsistent source objects, bounded-resource excess and
/// cancellation. Failed private stages are retained and reported, never auto-deleted.
pub fn prepare_git_seed(
    parent: &Path,
    resolved: &ResolvedRepositoryOverlay,
    source: &mut impl GitObjectSource,
    cancelled: impl Fn() -> bool,
) -> Result<PreparedGitSeed, SeedFailure> {
    #[cfg(target_os = "linux")]
    {
        staging::prepare(parent, resolved, source, &cancelled)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (parent, resolved, source, cancelled);
        Err(SeedFailure {
            reason: SeedError::Unsupported,
            residue: None,
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
