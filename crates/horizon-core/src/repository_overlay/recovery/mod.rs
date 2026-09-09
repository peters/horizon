//! Inert comparison preserving both snapshots, not a merge or recovery authorization.

mod compare;
mod identity;

use super::namespace::{NamespaceEntry, ResolvedRepositoryOverlay};
use std::fmt;

/// A relationship within one layer, not permission to choose or overwrite either side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeRelation {
    LocalOnly,
    RemoteOnly,
    SameChange,
    Divergent,
    /// Working-tree baselines differ because the two index entries differ.
    /// This remains explicit even if the final working entries are equal.
    DifferentIndexBases,
}

/// Borrowed literal states; absence means no leaf, never recursive directory deletion.
#[derive(Clone, Copy, Debug)]
pub struct EntryChange<'a> {
    pub before: Option<&'a NamespaceEntry>,
    pub after: Option<&'a NamespaceEntry>,
}

/// One changed or baseline-incompatible path, with both sides retained for inspection.
pub struct PathComparison<'a> {
    pub path: &'a str,
    pub relation: ChangeRelation,
    pub local: EntryChange<'a>,
    pub remote: EntryChange<'a>,
}

impl fmt::Debug for PathComparison<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PathComparison")
            .field("relation", &self.relation)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonSide {
    Local,
    Remote,
}

/// A leaf conflicts with the other side's descendant; both differ from the common Git base.
/// This includes inherited index changes when examining the working-tree namespace.
pub struct StructuralConflict<'a> {
    pub ancestor: &'a str,
    pub descendant: &'a str,
    pub ancestor_side: ComparisonSide,
}

impl fmt::Debug for StructuralConflict<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StructuralConflict")
            .field("ancestor_side", &self.ancestor_side)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub struct LayerComparison<'a> {
    paths: Vec<PathComparison<'a>>,
    structural_conflicts: Vec<StructuralConflict<'a>>,
}

impl<'a> LayerComparison<'a> {
    /// Byte-ordered paths. Paths unchanged on both sides with equal baselines are omitted.
    #[must_use]
    pub fn paths(&self) -> &[PathComparison<'a>] {
        &self.paths
    }

    /// Local ancestors first, then remote ancestors; each group is byte-ordered.
    #[must_use]
    pub fn structural_conflicts(&self) -> &[StructuralConflict<'a>] {
        &self.structural_conflicts
    }
}

/// Borrows both complete original bundles and namespaces without cloning payloads.
/// No conflict-free result promises safe composition: combining independent links,
/// for example, still requires complete topology validation by a separately authorized writer.
pub struct RecoveryComparison<'a> {
    local: &'a ResolvedRepositoryOverlay,
    remote: &'a ResolvedRepositoryOverlay,
    index: LayerComparison<'a>,
    working_tree: LayerComparison<'a>,
}

impl<'a> RecoveryComparison<'a> {
    #[must_use]
    pub fn local(&self) -> &'a ResolvedRepositoryOverlay {
        self.local
    }

    #[must_use]
    pub fn remote(&self) -> &'a ResolvedRepositoryOverlay {
        self.remote
    }

    #[must_use]
    pub fn index(&self) -> &LayerComparison<'a> {
        &self.index
    }

    #[must_use]
    pub fn working_tree(&self) -> &LayerComparison<'a> {
        &self.working_tree
    }
}

impl fmt::Debug for RecoveryComparison<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoveryComparison")
            .field("index_paths", &self.index.paths.len())
            .field("working_paths", &self.working_tree.paths.len())
            .finish_non_exhaustive()
    }
}

/// Compare an exact common source/base without filesystem, process or transport I/O.
/// Index baselines are the common Git tree; working baselines are each side's index.
/// Git identity comparisons do not newly read or verify regular base blob payloads.
/// No source ownership, capture coherence, secrecy, merge, recovery or durability is proved.
/// Both inputs remain caller-owned on success, failure and cancellation. Run off the UI
/// thread: at most 256 MiB of distinct verified overlay bytes are hashed, once per digest.
/// Cancellation is checked between traversal steps and before/after hashing; a single
/// bounded native blob hash (at most 64 MiB) is not interruptible or wall-time limited.
/// Per layer, paths/metadata are bounded by four existing namespace budgets and structural
/// conflicts by twice the namespace entry bound. Report paths and payloads are borrowed.
/// # Errors
/// Rejects source/base mismatch, inconsistent payload identity, bounded-resource excess
/// and cancellation. Diagnostics redact repository, path, content and digest values.
pub fn compare_recovery<'a>(
    local: &'a ResolvedRepositoryOverlay,
    remote: &'a ResolvedRepositoryOverlay,
    cancelled: impl Fn() -> bool,
) -> Result<RecoveryComparison<'a>, RecoveryComparisonError> {
    compare::run(local, remote, &cancelled)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RecoveryComparisonError {
    #[error("repository comparison requires the exact same source identity")]
    SourceMismatch,
    #[error("repository comparison requires consistent common base identities")]
    BaseMismatch,
    #[error("repository comparison encountered inconsistent content identity")]
    Object,
    #[error("repository comparison exceeds a supported bound")]
    Limit,
    #[error("repository comparison was cancelled")]
    Cancelled,
}

fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), RecoveryComparisonError> {
    if cancelled() {
        Err(RecoveryComparisonError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
