//! Named publication uses exclusive permanent digest slots, never reclaimable locks.
//! Stable trusted ownership is required; this is not protection from the worker owner.

use super::{ArtifactDigest, CONFINED, Directory, Error, RegularFileRead, RepositoryReadError, codec, storage_error};
use rustix::fs::{Mode, OFlags, mkdirat, openat2, renameat};
use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::MetadataExt,
};

const PENDING: &str = "pending";
const RECORD: &str = "record.hzov";
const PRIVATE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);

struct Slot(File);

impl Slot {
    fn open(parent: &Directory, digest: &ArtifactDigest) -> Result<Option<Self>, Error> {
        parent.verify()?;
        let file = match openat2(
            &parent.handle,
            digest.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        let metadata = file.metadata().map_err(|_| Error::UnsafeDirectory)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.nlink() == 0
        {
            return Err(Error::UnsafeDirectory);
        }
        Ok(Some(Self(file)))
    }

    fn verify(&self, parent: &Directory, digest: &ArtifactDigest) -> Result<(), Error> {
        let current = Self::open(parent, digest)?.ok_or(Error::Conflict)?;
        if !same_inode(&self.0, &current.0)? {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    fn read(&self, parent: &Directory, digest: &ArtifactDigest) -> Result<RegularFileRead, Error> {
        self.verify(parent, digest)?;
        // A partial claim, including an unexpected pending name, is not absence.
        match openat2(
            &self.0,
            PENDING,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        ) {
            Err(rustix::io::Errno::NOENT) => {}
            _ => return Err(Error::Conflict),
        }
        let record = parent
            .reader
            .read_private_file(
                &format!("{}/{RECORD}", digest.as_str()),
                codec::MAX_ENCODED_BUNDLE_BYTES,
            )
            .map_err(|error| match error {
                RepositoryReadError::Missing => Error::Conflict,
                other => Error::Read(other),
            })?;
        self.verify(parent, digest)?;
        Ok(record)
    }

    fn resynchronize(
        &self,
        parent: &Directory,
        digest: &ArtifactDigest,
        bytes: &[u8],
        expected: Option<&File>,
        sync: &mut impl FnMut(&File) -> io::Result<()>,
    ) -> Result<(), Error> {
        let record = self.read(parent, digest)?;
        if record.bytes != bytes {
            return Err(Error::Conflict);
        }
        if let Some(file) = expected
            && !same_inode(file, &record.file)?
        {
            return Err(Error::Conflict);
        }
        for file in [&record.file, &self.0, &parent.handle] {
            sync(file).map_err(|_| Error::WriteFailed)?;
        }
        let verified = self.read(parent, digest)?;
        if verified.bytes != bytes || !same_inode(&record.file, &verified.file)? {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

fn same_inode(left: &File, right: &File) -> Result<bool, Error> {
    let left = left.metadata().map_err(|_| Error::WriteFailed)?;
    let right = right.metadata().map_err(|_| Error::WriteFailed)?;
    Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
}

pub(super) fn read(parent: &Directory, digest: &ArtifactDigest) -> Result<Option<RegularFileRead>, Error> {
    Slot::open(parent, digest)?
        .map(|slot| slot.read(parent, digest))
        .transpose()
}

pub(super) fn put(parent: &Directory, digest: &ArtifactDigest, bytes: &[u8]) -> Result<(), Error> {
    put_with(
        parent,
        digest,
        bytes,
        File::sync_all,
        |root, name| mkdirat(root, name, PRIVATE),
        |slot| renameat(slot, PENDING, slot, RECORD),
    )
}

fn put_with(
    parent: &Directory,
    digest: &ArtifactDigest,
    bytes: &[u8],
    mut sync: impl FnMut(&File) -> io::Result<()>,
    mut claim: impl FnMut(&File, &str) -> Result<(), rustix::io::Errno>,
    mut publish: impl FnMut(&File) -> Result<(), rustix::io::Errno>,
) -> Result<(), Error> {
    parent.verify()?;
    match claim(&parent.handle, digest.as_str()) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {
            return Slot::open(parent, digest)?
                .ok_or(Error::Conflict)?
                .resynchronize(parent, digest, bytes, None, &mut sync);
        }
        // An ambiguous NFS mkdir error never confers ownership of a possible slot.
        Err(error) => return Err(storage_error(error)),
    }
    let slot = Slot::open(parent, digest)?.ok_or(Error::Conflict)?;
    sync(&parent.handle).map_err(|_| Error::WriteFailed)?;
    let mut file = File::from(
        openat2(
            &slot.0,
            PENDING,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            CONFINED,
        )
        .map_err(storage_error)?,
    );
    let metadata = file.metadata().map_err(|_| Error::WriteFailed)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o600
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(Error::WriteFailed);
    }
    file.write_all(bytes).map_err(|_| Error::WriteFailed)?;
    sync(&file).map_err(|_| Error::WriteFailed)?;
    slot.verify(parent, digest)?;
    // Only this successful fresh claim owner can rename. Existing slots never
    // take this path, so an existing complete or partial claim is not overwritten.
    // Even an ambiguous rename error returns failure, without retrying the rename.
    publish(&slot.0).map_err(storage_error)?;
    slot.resynchronize(parent, digest, bytes, Some(&file), &mut sync)
}

#[cfg(test)]
mod tests;
