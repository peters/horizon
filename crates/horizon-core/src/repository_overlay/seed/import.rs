use super::{
    GitObjectSource, GitObjectStream, MAX_BYTES, MAX_OBJECTS, ResolvedRepositoryOverlay, SeedError as Error,
    staging::check_cancel,
};
use crate::repository_overlay::{MAX_CHANGES, MAX_METADATA_BYTES};
use git2::{ObjectType, Odb, Oid, Repository};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};

pub(super) struct Importer<'a, 'repository, S, C> {
    database: &'a Odb<'repository>,
    source: &'a mut S,
    cancelled: &'a C,
    seen: BTreeMap<Oid, (ObjectType, u64)>,
    metadata: u64,
    bytes: u64,
}

impl<'a, 'repository, S: GitObjectSource, C: Fn() -> bool> Importer<'a, 'repository, S, C> {
    pub(super) fn new(database: &'a Odb<'repository>, source: &'a mut S, cancelled: &'a C) -> Self {
        Self {
            database,
            source,
            cancelled,
            seen: BTreeMap::new(),
            metadata: 0,
            bytes: 0,
        }
    }

    pub(super) fn objects(&self) -> usize {
        self.seen.len()
    }

    pub(super) fn base(&mut self, repository: &Repository, resolved: &ResolvedRepositoryOverlay) -> Result<(), Error> {
        self.object(resolved.base_commit(), ObjectType::Commit, None)?;
        let commit = repository
            .find_commit(resolved.base_commit())
            .map_err(|_| Error::Object)?;
        if commit.tree_id() != resolved.base_tree() {
            return Err(Error::Object);
        }
        let mut pending = vec![commit.tree_id()];
        while let Some(id) = pending.pop() {
            let fresh = !self.seen.contains_key(&id);
            self.object(id, ObjectType::Tree, None)?;
            if !fresh {
                continue;
            }
            let tree = repository.find_tree(id).map_err(|_| Error::Object)?;
            for entry in &tree {
                match entry.filemode_raw() {
                    0o040_000 => pending.push(entry.id()),
                    0o100_644 | 0o100_755 | 0o120_000 => {
                        self.object(entry.id(), ObjectType::Blob, None)?;
                    }
                    _ => return Err(Error::Object),
                }
            }
            if pending.len() > MAX_CHANGES {
                return Err(Error::Limit);
            }
        }
        Ok(())
    }

    pub(super) fn object(&mut self, id: Oid, kind: ObjectType, bytes: Option<u64>) -> Result<Oid, Error> {
        check_cancel(self.cancelled)?;
        if let Some((actual, length)) = self.seen.get(&id) {
            return if *actual == kind && bytes.is_none_or(|bytes| bytes == *length) {
                Ok(id)
            } else {
                Err(Error::Object)
            };
        }
        let stream = self.source.open(id)?;
        if stream.kind != kind || bytes.is_some_and(|bytes| bytes != stream.bytes) {
            return Err(Error::Object);
        }
        let length = stream.bytes;
        charge(self.seen.len(), &mut self.bytes, &mut self.metadata, kind, length)?;
        copy(self.database, stream, id, self.cancelled)?;
        self.seen.insert(id, (kind, length));
        Ok(id)
    }

    pub(super) fn blob(&mut self, bytes: &[u8]) -> Result<Oid, Error> {
        check_cancel(self.cancelled)?;
        let id = Oid::hash_object(ObjectType::Blob, bytes).map_err(|_| Error::Object)?;
        if let Some((kind, length)) = self.seen.get(&id) {
            return if *kind == ObjectType::Blob && *length == bytes.len() as u64 {
                Ok(id)
            } else {
                Err(Error::Object)
            };
        }
        let length = bytes.len() as u64;
        charge(
            self.seen.len(),
            &mut self.bytes,
            &mut self.metadata,
            ObjectType::Blob,
            length,
        )?;
        copy(
            self.database,
            GitObjectStream {
                kind: ObjectType::Blob,
                bytes: length,
                reader: Box::new(bytes),
            },
            id,
            self.cancelled,
        )?;
        self.seen.insert(id, (ObjectType::Blob, length));
        Ok(id)
    }
}

pub(super) fn charge(
    objects: usize,
    total: &mut u64,
    metadata: &mut u64,
    kind: ObjectType,
    bytes: u64,
) -> Result<(), Error> {
    if objects >= MAX_OBJECTS {
        return Err(Error::Limit);
    }
    *total = total
        .checked_add(bytes)
        .filter(|n| *n <= MAX_BYTES)
        .ok_or(Error::Limit)?;
    if matches!(kind, ObjectType::Commit | ObjectType::Tree) {
        *metadata = metadata
            .checked_add(bytes)
            .filter(|n| *n <= MAX_METADATA_BYTES as u64)
            .ok_or(Error::Limit)?;
    }
    Ok(())
}

fn copy(
    database: &Odb<'_>,
    mut stream: GitObjectStream<'_>,
    id: Oid,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Error> {
    let length = usize::try_from(stream.bytes).map_err(|_| Error::Limit)?;
    let mut writer = database.writer(length, stream.kind).map_err(|_| Error::Storage)?;
    let mut remaining = stream.bytes;
    let mut buffer = [0u8; 16 * 1024];
    while remaining > 0 {
        check_cancel(cancelled)?;
        let limit = usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| Error::Limit)?;
        let count = match stream.reader.read(&mut buffer[..limit]) {
            Ok(0) => return Err(Error::Object),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Source),
        };
        writer.write_all(&buffer[..count]).map_err(|_| Error::Storage)?;
        remaining -= count as u64;
    }
    check_cancel(cancelled)?;
    let mut tail = [0];
    loop {
        check_cancel(cancelled)?;
        match stream.reader.read(&mut tail) {
            Ok(0) => break,
            Ok(_) => return Err(Error::Object),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(Error::Source),
        }
    }
    if writer.finalize().map_err(|_| Error::Storage)? != id {
        return Err(Error::Object);
    }
    Ok(())
}
