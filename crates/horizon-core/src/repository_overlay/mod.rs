//! Inert, exact-base repository overlay metadata, not permission to capture or export files.
//! Index changes are relative to the base; working-tree changes are relative to that index.
//! Capture/apply must separately verify real filesystem topology, content hashes and approval.

pub mod bundle;
pub mod capture;
pub mod checkout;
pub mod materialize;
pub mod namespace;
mod paths;
pub mod reader;
pub mod recovery;
pub mod retained_setup;
pub mod seed;
#[cfg(target_os = "linux")]
mod storage;

use crate::cloud_run::{ArtifactDigest, GitSource};
use std::{collections::BTreeMap, fmt};

const MAX_CHANGES: usize = 16_384;
const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_CONTENT_BYTES: u64 = 16 * 1024 * 1024 * 1024;

/// A file replacement, literal relative symlink, or removal of a file/link (never recursive).
/// Index LFS pointer bytes and hydrated working-tree bytes have separate file identities.
#[derive(Clone, Eq, PartialEq)]
pub enum OverlayContent {
    Remove,
    File {
        sha256: ArtifactDigest,
        bytes: u64,
        executable: bool,
    },
    Symlink {
        target: String,
    },
}

impl fmt::Debug for OverlayContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remove => formatter.write_str("Remove"),
            Self::File { bytes, executable, .. } => formatter
                .debug_struct("File")
                .field("bytes", bytes)
                .field("executable", executable)
                .finish_non_exhaustive(),
            Self::Symlink { .. } => formatter.debug_struct("Symlink").finish_non_exhaustive(),
        }
    }
}

/// One checked path/content description. Contains no regular-file bytes or host paths.
#[derive(Clone, Eq, PartialEq)]
pub struct OverlayChange {
    path: String,
    content: OverlayContent,
}

impl OverlayChange {
    /// Validate metadata without following links or accessing a filesystem.
    /// Paths use a normalized UTF-8 POSIX subset; unsupported names are rejected, not renamed.
    /// # Errors
    /// Rejects malformed/excluded paths, unsafe lexical link targets and oversized files.
    pub fn new(path: String, content: OverlayContent) -> Result<Self, OverlayPlanError> {
        paths::validate(&path)?;
        match &content {
            OverlayContent::File { bytes, .. } if *bytes > MAX_CONTENT_BYTES => {
                return Err(OverlayPlanError::ContentLimit);
            }
            OverlayContent::Symlink { target } => paths::validate_link(&path, target)?,
            _ => {}
        }
        Ok(Self { path, content })
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn content(&self) -> &OverlayContent {
        &self.content
    }
}

impl fmt::Debug for OverlayChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OverlayChange")
            .field("content", &self.content)
            .finish_non_exhaustive()
    }
}

/// A bounded two-layer plan tied to an exact commit, with deterministic path ordering.
/// Renames are remove/add pairs; an untracked addition exists only in `working_tree`.
/// This neither snapshots a repository nor certifies base-tree links or secret-free content.
#[derive(Clone, Eq, PartialEq)]
pub struct RepositoryOverlayPlan {
    source: GitSource,
    index: Vec<OverlayChange>,
    working_tree: Vec<OverlayChange>,
    content_bytes: u64,
}

impl RepositoryOverlayPlan {
    /// Build an inert plan from metadata selected by a separate approval/capture boundary.
    /// Each layer describes its resulting file/link states, not sequential filesystem writes.
    /// A future applier must safely process removals before additions and recheck real nodes.
    /// # Errors
    /// Rejects invalid source identity, duplicate/overlapping paths and aggregate limit excess.
    pub fn new(
        source: GitSource,
        index: impl IntoIterator<Item = OverlayChange>,
        working_tree: impl IntoIterator<Item = OverlayChange>,
    ) -> Result<Self, OverlayPlanError> {
        source.validate().map_err(|_| OverlayPlanError::InvalidSource)?;
        let mut budget = Budget::default();
        budget.add_metadata(source.repository.len())?;
        budget.add_metadata(source.commit.as_str().len())?;
        budget.add_metadata(source.branch.as_ref().map_or(0, String::len))?;
        let index = collect_layer(index, &mut budget)?;
        let working_tree = collect_layer(working_tree, &mut budget)?;
        validate_parents(&index, &[])?;
        validate_parents(&[], &working_tree)?;
        validate_parents(&index, &working_tree)?;
        Ok(Self {
            source,
            index,
            working_tree,
            content_bytes: budget.content,
        })
    }

    #[must_use]
    pub fn source(&self) -> &GitSource {
        &self.source
    }

    #[must_use]
    pub fn index(&self) -> &[OverlayChange] {
        &self.index
    }

    #[must_use]
    pub fn working_tree(&self) -> &[OverlayChange] {
        &self.working_tree
    }

    /// Conservative payload budget across both layers; shared digests are counted twice.
    #[must_use]
    pub fn content_bytes(&self) -> u64 {
        self.content_bytes
    }
}

impl fmt::Debug for RepositoryOverlayPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepositoryOverlayPlan")
            .field("index_changes", &self.index.len())
            .field("working_tree_changes", &self.working_tree.len())
            .field("content_bytes", &self.content_bytes)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Budget {
    changes: usize,
    metadata: usize,
    content: u64,
}

impl Budget {
    fn add_metadata(&mut self, bytes: usize) -> Result<(), OverlayPlanError> {
        self.metadata = self
            .metadata
            .checked_add(bytes)
            .filter(|total| *total <= MAX_METADATA_BYTES)
            .ok_or(OverlayPlanError::MetadataLimit)?;
        Ok(())
    }

    fn add(&mut self, change: &OverlayChange) -> Result<(), OverlayPlanError> {
        if self.changes == MAX_CHANGES {
            return Err(OverlayPlanError::ChangeLimit);
        }
        self.changes += 1;
        self.add_metadata(change.path.len())?;
        let bytes = match &change.content {
            OverlayContent::Remove => 0,
            OverlayContent::File { bytes, sha256, .. } => {
                self.add_metadata(sha256.as_str().len())?;
                *bytes
            }
            OverlayContent::Symlink { target } => {
                self.add_metadata(target.len())?;
                u64::try_from(target.len()).map_err(|_| OverlayPlanError::ContentLimit)?
            }
        };
        self.content = self
            .content
            .checked_add(bytes)
            .filter(|total| *total <= MAX_CONTENT_BYTES)
            .ok_or(OverlayPlanError::ContentLimit)?;
        Ok(())
    }
}

fn collect_layer(
    changes: impl IntoIterator<Item = OverlayChange>,
    budget: &mut Budget,
) -> Result<Vec<OverlayChange>, OverlayPlanError> {
    let mut layer = Vec::new();
    for change in changes {
        budget.add(&change)?;
        layer.push(change);
    }
    layer.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    if layer.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(OverlayPlanError::DuplicatePath);
    }
    Ok(layer)
}

fn validate_parents(index: &[OverlayChange], working_tree: &[OverlayChange]) -> Result<(), OverlayPlanError> {
    let mut present = BTreeMap::new();
    for change in index.iter().chain(working_tree) {
        present.insert(change.path.as_str(), !matches!(change.content, OverlayContent::Remove));
    }
    for (path, exists) in &present {
        if !*exists {
            continue;
        }
        let mut parent = *path;
        while let Some((next, _)) = parent.rsplit_once('/') {
            if present.get(next) == Some(&true) {
                return Err(OverlayPlanError::ParentCollision);
            }
            parent = next;
        }
    }
    Ok(())
}

/// Diagnostics omit repository, path, link target, digest and file content values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OverlayPlanError {
    #[error("overlay source is not a valid exact repository identity")]
    InvalidSource,
    #[error("overlay path is not a supported normalized repository-relative path")]
    InvalidPath,
    #[error("overlay path is excluded by the repository transfer policy")]
    ExcludedPath,
    #[error("overlay symlink target does not remain within allowed repository paths")]
    InvalidLink,
    #[error("overlay layer contains multiple changes for the same path")]
    DuplicatePath,
    #[error("overlay file or symlink conflicts with a descendant path")]
    ParentCollision,
    #[error("overlay exceeds its total change limit")]
    ChangeLimit,
    #[error("overlay exceeds its metadata budget")]
    MetadataLimit,
    #[error("overlay exceeds its content budget")]
    ContentLimit,
}

#[cfg(test)]
mod tests;
