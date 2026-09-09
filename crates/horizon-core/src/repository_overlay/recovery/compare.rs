use super::{
    ChangeRelation, ComparisonSide, EntryChange, LayerComparison, PathComparison, RecoveryComparison,
    RecoveryComparisonError as Error, StructuralConflict, check_cancel, identity::Identities,
};
use crate::repository_overlay::{
    MAX_CHANGES, MAX_METADATA_BYTES,
    bundle::RepositoryOverlayBundle,
    namespace::{RepositoryNamespace, ResolvedRepositoryOverlay},
};
use std::{collections::BTreeSet, ops::Bound};

pub(super) fn run<'a>(
    local: &'a ResolvedRepositoryOverlay,
    remote: &'a ResolvedRepositoryOverlay,
    cancelled: &impl Fn() -> bool,
) -> Result<RecoveryComparison<'a>, Error> {
    check_cancel(cancelled)?;
    if local.bundle().plan().source() != remote.bundle().plan().source() {
        return Err(Error::SourceMismatch);
    }
    if local.base_commit() != remote.base_commit()
        || local.base_tree() != remote.base_tree()
        || local.base().entries().len() != remote.base().entries().len()
    {
        return Err(Error::BaseMismatch);
    }
    for (a, b) in local.base().entries().zip(remote.base().entries()) {
        check_cancel(cancelled)?;
        if a != b {
            return Err(Error::BaseMismatch);
        }
    }
    let mut identities = Identities::new(cancelled);
    let index = layer(
        Side {
            before: local.base(),
            after: local.index(),
            bundle: local.bundle(),
        },
        Side {
            before: remote.base(),
            after: remote.index(),
            bundle: remote.bundle(),
        },
        local.base(),
        &mut identities,
        cancelled,
    )?;
    let working_tree = layer(
        Side {
            before: local.index(),
            after: local.working_tree(),
            bundle: local.bundle(),
        },
        Side {
            before: remote.index(),
            after: remote.working_tree(),
            bundle: remote.bundle(),
        },
        local.base(),
        &mut identities,
        cancelled,
    )?;
    check_cancel(cancelled)?;
    Ok(RecoveryComparison {
        local,
        remote,
        index,
        working_tree,
    })
}

#[derive(Clone, Copy)]
struct Side<'a> {
    before: &'a RepositoryNamespace,
    after: &'a RepositoryNamespace,
    bundle: &'a RepositoryOverlayBundle,
}

fn layer<'a>(
    local: Side<'a>,
    remote: Side<'a>,
    base: &'a RepositoryNamespace,
    identities: &mut Identities<'a, '_, impl Fn() -> bool>,
    cancelled: &impl Fn() -> bool,
) -> Result<LayerComparison<'a>, Error> {
    let mut paths = BTreeSet::new();
    let mut bytes = 0usize;
    for namespace in [local.before, local.after, remote.before, remote.after] {
        for (path, _) in namespace.entries() {
            check_cancel(cancelled)?;
            if paths.insert(path) {
                bytes = bytes
                    .checked_add(path.len())
                    .filter(|n| *n <= 4 * MAX_METADATA_BYTES)
                    .ok_or(Error::Limit)?;
                if paths.len() > 4 * MAX_CHANGES {
                    return Err(Error::Limit);
                }
            }
        }
    }
    let mut result = LayerComparison::default();
    let mut local_leaves = BTreeSet::new();
    let mut remote_leaves = BTreeSet::new();
    for path in paths {
        check_cancel(cancelled)?;
        let a = EntryChange {
            before: local.before.entry(path),
            after: local.after.entry(path),
        };
        let b = EntryChange {
            before: remote.before.entry(path),
            after: remote.after.entry(path),
        };
        let same_base = identities.equal(a.before, local.bundle, b.before, remote.bundle)?;
        let ac = !identities.equal(a.before, local.bundle, a.after, local.bundle)?;
        let bc = !identities.equal(b.before, remote.bundle, b.after, remote.bundle)?;
        let relation = if same_base {
            match (ac, bc) {
                (false, false) => None,
                (true, false) => Some(ChangeRelation::LocalOnly),
                (false, true) => Some(ChangeRelation::RemoteOnly),
                (true, true) => Some(if identities.equal(a.after, local.bundle, b.after, remote.bundle)? {
                    ChangeRelation::SameChange
                } else {
                    ChangeRelation::Divergent
                }),
            }
        } else {
            Some(ChangeRelation::DifferentIndexBases)
        };
        if let Some(relation) = relation {
            result.paths.push(PathComparison {
                path,
                relation,
                local: a,
                remote: b,
            });
        }
        if a.after.is_some() && !identities.equal(a.after, local.bundle, base.entry(path), local.bundle)? {
            local_leaves.insert(path);
        }
        if b.after.is_some() && !identities.equal(b.after, remote.bundle, base.entry(path), local.bundle)? {
            remote_leaves.insert(path);
        }
    }
    conflicts(
        &local_leaves,
        &remote_leaves,
        ComparisonSide::Local,
        &mut result,
        cancelled,
    )?;
    conflicts(
        &remote_leaves,
        &local_leaves,
        ComparisonSide::Remote,
        &mut result,
        cancelled,
    )?;
    Ok(result)
}

fn conflicts<'a>(
    ancestors: &BTreeSet<&'a str>,
    descendants: &BTreeSet<&'a str>,
    side: ComparisonSide,
    result: &mut LayerComparison<'a>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Error> {
    for ancestor in ancestors {
        check_cancel(cancelled)?;
        let prefix = format!("{ancestor}/");
        for descendant in descendants.range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded)) {
            check_cancel(cancelled)?;
            if !descendant.starts_with(&prefix) {
                break;
            }
            if result.structural_conflicts.len() == 2 * MAX_CHANGES {
                return Err(Error::Limit);
            }
            result.structural_conflicts.push(StructuralConflict {
                ancestor,
                descendant,
                ancestor_side: side,
            });
        }
    }
    Ok(())
}
