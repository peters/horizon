use super::{Budget, NamespaceEntry, NamespaceError as Error, NamespaceFile, RepositoryNamespace, links};
use crate::repository_overlay::{OverlayChange, OverlayContent, paths};

pub(super) fn apply(
    previous: &RepositoryNamespace,
    changes: &[OverlayChange],
    work: &mut links::Work,
) -> Result<RepositoryNamespace, Error> {
    let mut next = previous.clone();
    for change in changes {
        if matches!(change.content(), OverlayContent::Remove) {
            // Exact leaves only. A directory name never grants recursive removal.
            if has_descendant(&previous.entries, change.path()) {
                return Err(Error::Topology);
            }
            next.entries.remove(change.path());
        }
    }
    for change in changes {
        let entry = match change.content() {
            OverlayContent::Remove => continue,
            OverlayContent::File {
                sha256,
                bytes,
                executable,
            } => NamespaceEntry::File {
                source: NamespaceFile::Overlay {
                    sha256: sha256.clone(),
                    bytes: *bytes,
                },
                executable: *executable,
            },
            OverlayContent::Symlink { target } => NamespaceEntry::Symlink { target: target.clone() },
        };
        next.entries.insert(change.path().to_owned(), entry);
    }
    validate(&mut next, work)?;
    Ok(next)
}

pub(super) fn has_descendant(entries: &std::collections::BTreeMap<String, NamespaceEntry>, path: &str) -> bool {
    let prefix = format!("{path}/");
    entries
        .range(prefix.clone()..)
        .next()
        .is_some_and(|(name, _)| name.starts_with(&prefix))
}

pub(super) fn validate(namespace: &mut RepositoryNamespace, work: &mut links::Work) -> Result<(), Error> {
    let mut budget = Budget::default();
    let mut previous = "";
    for (path, entry) in &namespace.entries {
        paths::validate(path)?;
        let (target, bytes) = match entry {
            NamespaceEntry::File { source, .. } => (None, source.bytes()),
            NamespaceEntry::Symlink { target } => {
                paths::validate_link(path, target)?;
                (Some(target.as_str()), 0)
            }
        };
        budget.add(path, target, bytes)?;
        let shared = path.bytes().zip(previous.bytes()).take_while(|(a, b)| a == b).count();
        for (boundary, _) in path.match_indices('/') {
            let parent = &path[..boundary];
            // Sorted leaves share contiguous directory prefixes. Charge each
            // implicit directory and check its identity once, without allocating
            // all ancestor strings or repeating shared-prefix tree lookups.
            if boundary >= shared {
                if namespace.entries.contains_key(parent) {
                    return Err(Error::Topology);
                }
                budget.add(parent, None, 0)?;
            }
        }
        previous = path;
    }
    links::validate(namespace, work)?;
    namespace.logical_bytes = budget.logical;
    Ok(())
}
