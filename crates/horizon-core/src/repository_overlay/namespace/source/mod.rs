mod objects;
mod records;

use super::{Budget, NamespaceEntry, NamespaceError as Error, NamespaceFile, RepositoryNamespace};
use crate::repository_overlay::{
    MAX_METADATA_BYTES, paths,
    seed::{GitObjectInspector, SeedError},
};
use git2::{ObjectType, Oid};
use std::collections::BTreeSet;

pub(super) fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(SeedError::Cancelled.into())
    } else {
        Ok(())
    }
}

pub(super) fn read(
    source: &mut impl GitObjectInspector,
    commit: Oid,
    cancelled: &impl Fn() -> bool,
) -> Result<(Oid, RepositoryNamespace), Error> {
    let mut objects = objects::Objects::new(source, cancelled);
    let root = records::commit_tree(&objects.read(commit, ObjectType::Commit, MAX_METADATA_BYTES)?)?;
    let mut pending = vec![(String::new(), root)];
    let mut namespace = RepositoryNamespace::default();
    let mut budget = Budget::default();
    let mut seen = BTreeSet::new();
    while let Some((prefix, id)) = pending.pop() {
        let bytes = objects.read(id, ObjectType::Tree, MAX_METADATA_BYTES)?;
        if !prefix.is_empty() && bytes.is_empty() {
            return Err(Error::UnsupportedNode);
        }
        let mut records = records::Tree::new(&bytes);
        while let Some(entry) = records.next()? {
            check_cancel(cancelled)?;
            let length = prefix.len() + usize::from(!prefix.is_empty()) + entry.name.len();
            if length > paths::MAX_PATH_BYTES {
                return Err(Error::Limit);
            }
            let path = if prefix.is_empty() {
                entry.name.to_owned()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            paths::validate(&path)?;
            if entry.kind == records::Kind::Directory {
                budget.add(&path, None, 0)?;
                if !seen.insert(path.clone()) {
                    return Err(Error::Topology);
                }
                pending.push((path, entry.object));
            } else {
                let node = leaf(&mut objects, entry.object, entry.kind)?;
                let (target, bytes) = match &node {
                    NamespaceEntry::File { source, .. } => (None, source.bytes()),
                    NamespaceEntry::Symlink { target } => (Some(target.as_str()), 0),
                };
                budget.add(&path, target, bytes)?;
                if !seen.insert(path.clone()) || namespace.entries.insert(path, node).is_some() {
                    return Err(Error::Topology);
                }
            }
        }
    }
    Ok((root, namespace))
}

fn leaf<S: GitObjectInspector, C: Fn() -> bool>(
    objects: &mut objects::Objects<'_, S, C>,
    object: Oid,
    kind: records::Kind,
) -> Result<NamespaceEntry, Error> {
    if kind == records::Kind::Link {
        let bytes = objects.read(object, ObjectType::Blob, paths::MAX_PATH_BYTES)?;
        Ok(NamespaceEntry::Symlink {
            target: String::from_utf8(bytes).map_err(|_| Error::UnsupportedNode)?,
        })
    } else {
        let metadata = objects.inspect(object)?;
        if metadata.kind != ObjectType::Blob {
            return Err(Error::Object);
        }
        Ok(NamespaceEntry::File {
            source: NamespaceFile::Base {
                object,
                bytes: metadata.bytes,
            },
            executable: kind == records::Kind::Executable,
        })
    }
}

#[cfg(test)]
mod tests;
