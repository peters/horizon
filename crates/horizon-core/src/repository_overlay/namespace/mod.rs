//! Compose exact-base, index and working-tree namespaces without applying or exporting them.

mod git;
mod links;
mod source;
mod tree;

use super::seed::{GitObjectInspector, SeedError};
use super::{MAX_CHANGES, MAX_CONTENT_BYTES, MAX_METADATA_BYTES, OverlayPlanError, bundle::RepositoryOverlayBundle};
use crate::cloud_run::ArtifactDigest;
use git2::{Oid, Repository};
use std::{collections::BTreeMap, fmt};

/// A file reference, not a promise that base bytes have been read or exported.
#[derive(Clone, Eq, PartialEq)]
pub enum NamespaceFile {
    Base { object: Oid, bytes: u64 },
    Overlay { sha256: ArtifactDigest, bytes: u64 },
}

impl NamespaceFile {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        match self {
            Self::Base { bytes, .. } | Self::Overlay { bytes, .. } => *bytes,
        }
    }
}

/// One final leaf. Directories are implicit; removals are never recursive nodes.
#[derive(Clone, Eq, PartialEq)]
pub enum NamespaceEntry {
    File { source: NamespaceFile, executable: bool },
    Symlink { target: String },
}

impl fmt::Debug for NamespaceEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { source, executable } => formatter
                .debug_struct("File")
                .field("bytes", &source.bytes())
                .field("executable", executable)
                .finish_non_exhaustive(),
            Self::Symlink { .. } => formatter.debug_struct("Symlink").finish_non_exhaustive(),
        }
    }
}

/// Immutable ordered leaves with validated complete-tree topology and effective links.
#[derive(Clone, Default)]
pub struct RepositoryNamespace {
    entries: BTreeMap<String, NamespaceEntry>,
    logical_bytes: u64,
}

impl RepositoryNamespace {
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&NamespaceEntry> {
        self.entries.get(path)
    }

    #[must_use]
    pub fn entries(&self) -> impl ExactSizeIterator<Item = (&str, &NamespaceEntry)> {
        self.entries.iter().map(|(path, entry)| (path.as_str(), entry))
    }

    /// Sum of regular-file lengths at every path, including repeated blob references.
    #[must_use]
    pub fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }
}

/// Owns the original verified overlay and complete namespaces, without duplicating payloads.
/// Only the resolver can construct this result. A later writer must revalidate base bytes
/// against their Git IDs and use a separately authorized, confined fresh destination.
pub struct ResolvedRepositoryOverlay {
    bundle: RepositoryOverlayBundle,
    base_commit: Oid,
    base_tree: Oid,
    base: RepositoryNamespace,
    index: RepositoryNamespace,
    working_tree: RepositoryNamespace,
}

impl ResolvedRepositoryOverlay {
    #[must_use]
    pub fn bundle(&self) -> &RepositoryOverlayBundle {
        &self.bundle
    }

    #[must_use]
    pub fn base_commit(&self) -> Oid {
        self.base_commit
    }

    #[must_use]
    pub fn base_tree(&self) -> Oid {
        self.base_tree
    }

    #[must_use]
    pub fn base(&self) -> &RepositoryNamespace {
        &self.base
    }

    #[must_use]
    pub fn index(&self) -> &RepositoryNamespace {
        &self.index
    }

    #[must_use]
    pub fn working_tree(&self) -> &RepositoryNamespace {
        &self.working_tree
    }
}

impl fmt::Debug for ResolvedRepositoryOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedRepositoryOverlay")
            .field("base_entries", &self.base.entries.len())
            .field("index_entries", &self.index.entries.len())
            .field("working_entries", &self.working_tree.entries.len())
            .finish_non_exhaustive()
    }
}

/// Resolve the bundle's exact SHA-1 commit directly, independently of current HEAD/index.
/// The caller supplies trusted Git metadata; this does not verify repository URL ownership
/// or authorize capture/export. No worktree reads, writes, filters, hooks or transport occur.
/// Regular base blobs remain references; only link payloads are read for topology validation.
/// Run off the UI thread. Bounds cover expanded namespaces and link work, not filesystem
/// latency or all native Git metadata allocations. No ready-checkout guarantee is returned.
/// # Errors
/// Rejects unsupported/corrupt base metadata, excluded paths, impossible topology, unsafe
/// effective links and expanded entry, metadata, logical-byte or link-work limit excess.
pub fn resolve_namespaces(
    base: &Repository,
    bundle: RepositoryOverlayBundle,
) -> Result<ResolvedRepositoryOverlay, NamespaceError> {
    let base_commit = Oid::from_str(bundle.plan().source().commit.as_str()).map_err(|_| NamespaceError::Base)?;
    compose(bundle, base_commit, git::read(base, base_commit)?, &|| false)
}

/// Resolve the exact base through an explicitly authorized raw-object inspector.
/// Regular blobs remain header-only references; commit, tree and link bytes are
/// length/type/hash verified within aggregate metadata and expanded namespace bounds.
/// Read metadata work, including repeated tree expansion, is capped at 8 MiB; a valid
/// repository can exceed this supported subset. Native source decoding needs its own caps.
/// Only supported SHA-1 record shapes are accepted; signatures are not authenticated.
/// The seed importer must still parse the complete commit before preparation succeeds.
/// No repository metadata configuration, worktree I/O, writes or export authority.
/// Source calls own their blocking/resource policy. Run off the UI thread; cancellation
/// is checked between object operations, chunks, tree records and composition phases.
/// # Errors
/// Rejects invalid or unsupported metadata, unsafe topology/links, bounded-resource
/// excess, cancellation and redacted source failures. No ready checkout is returned.
pub fn resolve_namespaces_from_source(
    source: &mut impl GitObjectInspector,
    bundle: RepositoryOverlayBundle,
    cancelled: impl Fn() -> bool,
) -> Result<ResolvedRepositoryOverlay, NamespaceError> {
    let base_commit = Oid::from_str(bundle.plan().source().commit.as_str()).map_err(|_| NamespaceError::Base)?;
    let original = source::read(source, base_commit, &cancelled)?;
    compose(bundle, base_commit, original, &cancelled)
}

fn compose(
    bundle: RepositoryOverlayBundle,
    base_commit: Oid,
    (base_tree, mut original): (Oid, RepositoryNamespace),
    cancelled: &impl Fn() -> bool,
) -> Result<ResolvedRepositoryOverlay, NamespaceError> {
    source::check_cancel(cancelled)?;
    let mut link_work = links::Work::default();
    tree::validate(&mut original, &mut link_work)?;
    source::check_cancel(cancelled)?;
    let index = tree::apply(&original, bundle.plan().index(), &mut link_work)?;
    source::check_cancel(cancelled)?;
    let working_tree = tree::apply(&index, bundle.plan().working_tree(), &mut link_work)?;
    source::check_cancel(cancelled)?;
    Ok(ResolvedRepositoryOverlay {
        bundle,
        base_commit,
        base_tree,
        base: original,
        index,
        working_tree,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NamespaceError {
    #[error("the exact supported repository base could not be read")]
    Base,
    #[error("repository object metadata is invalid or inconsistent")]
    Object,
    #[error("repository namespace contains an unsupported node")]
    UnsupportedNode,
    #[error("repository namespace has conflicting leaf and directory identities")]
    Topology,
    #[error("repository link resolution is unsafe or unsupported")]
    Link,
    #[error("expanded repository namespace exceeds a supported bound")]
    Limit,
    #[error(transparent)]
    Policy(#[from] OverlayPlanError),
    #[error(transparent)]
    Source(#[from] SeedError),
}

#[derive(Default)]
struct Budget {
    entries: usize,
    metadata: usize,
    logical: u64,
}

impl Budget {
    fn add(&mut self, path: &str, target: Option<&str>, bytes: u64) -> Result<(), NamespaceError> {
        self.entries = self
            .entries
            .checked_add(1)
            .filter(|n| *n <= MAX_CHANGES)
            .ok_or(NamespaceError::Limit)?;
        self.metadata = self
            .metadata
            .checked_add(path.len())
            .and_then(|n| n.checked_add(target.map_or(0, str::len)))
            .filter(|n| *n <= MAX_METADATA_BYTES)
            .ok_or(NamespaceError::Limit)?;
        self.logical = self
            .logical
            .checked_add(bytes)
            .filter(|n| *n <= MAX_CONTENT_BYTES)
            .ok_or(NamespaceError::Limit)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
