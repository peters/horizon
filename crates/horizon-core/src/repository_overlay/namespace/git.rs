use super::{Budget, NamespaceEntry, NamespaceError as Error, NamespaceFile, RepositoryNamespace};
use crate::repository_overlay::{MAX_METADATA_BYTES, paths};
use git2::{ObjectFormat, ObjectType, Odb, Oid, Repository};
use std::collections::BTreeSet;

pub(super) fn read(repository: &Repository, commit: Oid) -> Result<(Oid, RepositoryNamespace), Error> {
    if repository.object_format() != ObjectFormat::Sha1 {
        return Err(Error::Base);
    }
    let database = repository.odb().map_err(|_| Error::Base)?;
    verify_metadata(&database, commit, ObjectType::Commit)?;
    let commit = repository.find_commit(commit).map_err(|_| Error::Base)?;
    let root = commit.tree_id();
    let mut pending = vec![(String::new(), root)];
    let mut namespace = RepositoryNamespace::default();
    let mut budget = Budget::default();
    let mut seen = BTreeSet::new();
    while let Some((prefix, id)) = pending.pop() {
        verify_metadata(&database, id, ObjectType::Tree)?;
        let tree = repository.find_tree(id).map_err(|_| Error::Object)?;
        if !prefix.is_empty() && tree.is_empty() {
            return Err(Error::UnsupportedNode);
        }
        for entry in &tree {
            let name = entry.name().map_err(|_| Error::UnsupportedNode)?;
            if name.contains('/') {
                return Err(Error::UnsupportedNode);
            }
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            paths::validate(&path)?;
            if entry.filemode_raw() == 0o040_000 {
                budget.add(&path, None, 0)?;
                if !seen.insert(path.clone()) {
                    return Err(Error::Topology);
                }
                pending.push((path, entry.id()));
            } else {
                let node = leaf(&database, entry.id(), entry.filemode_raw())?;
                let (target, bytes) = match &node {
                    NamespaceEntry::File { source, .. } => (None, source.bytes()),
                    NamespaceEntry::Symlink { target } => (Some(target.as_str()), 0),
                };
                budget.add(&path, target, bytes)?;
                if !seen.insert(path.clone()) {
                    return Err(Error::Topology);
                }
                if namespace.entries.insert(path, node).is_some() {
                    return Err(Error::Topology);
                }
            }
        }
    }
    Ok((root, namespace))
}

fn verify_metadata(database: &Odb<'_>, id: Oid, kind: ObjectType) -> Result<(), Error> {
    let (bytes, actual) = database.read_header(id).map_err(|_| Error::Base)?;
    if bytes > MAX_METADATA_BYTES {
        return Err(Error::Limit);
    }
    if actual != kind {
        return Err(Error::Object);
    }
    let object = database.read(id).map_err(|_| Error::Object)?;
    if object.kind() != kind
        || object.len() != bytes
        || Oid::hash_object(kind, object.data()).map_err(|_| Error::Object)? != id
    {
        return Err(Error::Object);
    }
    Ok(())
}

fn leaf(database: &Odb<'_>, object: Oid, mode: i32) -> Result<NamespaceEntry, Error> {
    if !matches!(mode, 0o100_644 | 0o100_755 | 0o120_000) {
        return Err(Error::UnsupportedNode);
    }
    let (bytes, kind) = database.read_header(object).map_err(|_| Error::Object)?;
    if kind != ObjectType::Blob {
        return Err(Error::Object);
    }
    if mode == 0o120_000 {
        if bytes > paths::MAX_PATH_BYTES {
            return Err(Error::Limit);
        }
        let blob = database.read(object).map_err(|_| Error::Object)?;
        if blob.kind() != ObjectType::Blob
            || blob.len() != bytes
            || Oid::hash_object(ObjectType::Blob, blob.data()).map_err(|_| Error::Object)? != object
        {
            return Err(Error::Object);
        }
        Ok(NamespaceEntry::Symlink {
            target: std::str::from_utf8(blob.data())
                .map_err(|_| Error::UnsupportedNode)?
                .to_owned(),
        })
    } else {
        Ok(NamespaceEntry::File {
            source: NamespaceFile::Base {
                object,
                bytes: u64::try_from(bytes).map_err(|_| Error::Limit)?,
            },
            executable: mode == 0o100_755,
        })
    }
}
