use super::{Error, GitObjectInspector, MAX_METADATA_BYTES, SeedError, check_cancel};
use crate::repository_overlay::seed::GitObjectMetadata;
use git2::{ObjectType, Oid};
use std::io::{self, Read};

pub(super) struct Objects<'a, S, C> {
    source: &'a mut S,
    cancelled: &'a C,
    metadata: usize,
}

impl<'a, S: GitObjectInspector, C: Fn() -> bool> Objects<'a, S, C> {
    pub(super) fn new(source: &'a mut S, cancelled: &'a C) -> Self {
        Self {
            source,
            cancelled,
            metadata: 0,
        }
    }

    pub(super) fn inspect(&mut self, object: Oid) -> Result<GitObjectMetadata, Error> {
        check_cancel(self.cancelled)?;
        let result = self.source.inspect(object);
        check_cancel(self.cancelled)?;
        result.map_err(Error::from)
    }

    pub(super) fn read(&mut self, object: Oid, kind: ObjectType, limit: usize) -> Result<Vec<u8>, Error> {
        check_cancel(self.cancelled)?;
        let stream = self.source.open(object);
        check_cancel(self.cancelled)?;
        let mut stream = stream?;
        if stream.kind != kind {
            return Err(Error::Object);
        }
        let length = usize::try_from(stream.bytes).map_err(|_| Error::Limit)?;
        if length > limit {
            return Err(Error::Limit);
        }
        self.metadata = self
            .metadata
            .checked_add(length)
            .filter(|n| *n <= MAX_METADATA_BYTES)
            .ok_or(Error::Limit)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|_| Error::Limit)?;
        bytes.resize(length, 0);
        for chunk in bytes.chunks_mut(16 * 1024) {
            let mut offset = 0;
            while offset < chunk.len() {
                let count = read_chunk(&mut *stream.reader, &mut chunk[offset..], self.cancelled)?;
                if count == 0 {
                    return Err(Error::Object);
                }
                offset += count;
            }
        }
        if read_chunk(&mut *stream.reader, &mut [0], self.cancelled)? != 0
            || Oid::hash_object(kind, &bytes).map_err(|_| Error::Object)? != object
        {
            return Err(Error::Object);
        }
        check_cancel(self.cancelled)?;
        Ok(bytes)
    }
}

fn read_chunk(reader: &mut dyn Read, bytes: &mut [u8], cancelled: &impl Fn() -> bool) -> Result<usize, Error> {
    loop {
        check_cancel(cancelled)?;
        let result = reader.read(bytes);
        check_cancel(cancelled)?;
        match result {
            Ok(count) => return Ok(count),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(SeedError::Source.into()),
        }
    }
}
