use super::{ArtifactDigest, BundleStoreError as Error, codec};
mod named;
use crate::repository_overlay::reader::{
    RepositoryReadError, SelectedRepositoryReader,
    linux::{RegularFileRead, Root},
};
use rustix::fs::{AtFlags, CWD, Mode, OFlags, ResolveFlags, linkat, openat2};
use std::{
    fs::File,
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(super) struct Directory {
    reader: Root,
    handle: File,
    named: bool,
}

impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(path)?.root;
        let directory = File::from(
            openat2(
                reader.handle(),
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(storage_error)?,
        );
        let result = Self {
            reader,
            handle: directory,
            named: false,
        };
        result.verify()?;
        Ok(result)
    }

    pub(super) fn open_named(path: &Path) -> Result<Self, Error> {
        let mut directory = Self::open(path)?;
        directory.named = true;
        Ok(directory)
    }

    fn verify(&self) -> Result<(), Error> {
        let metadata = self.handle.metadata().map_err(|_| Error::UnsafeDirectory)?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.nlink() == 0
        {
            return Err(Error::UnsafeDirectory);
        }
        Ok(())
    }

    pub(super) fn read(&self, digest: &ArtifactDigest) -> Result<Option<RegularFileRead>, Error> {
        if self.named {
            return named::read(self, digest);
        }
        self.verify()?;
        match self
            .reader
            .read_private_file(&name(digest), codec::MAX_ENCODED_BUNDLE_BYTES)
        {
            Ok(record) => Ok(Some(record)),
            Err(RepositoryReadError::Missing) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn put(&self, digest: &ArtifactDigest, bytes: &[u8]) -> Result<(), Error> {
        if self.named {
            return named::put(self, digest, bytes);
        }
        self.put_with_sync(digest, bytes, File::sync_all)
    }

    fn put_with_sync(
        &self,
        digest: &ArtifactDigest,
        bytes: &[u8],
        mut sync: impl FnMut(&File) -> io::Result<()>,
    ) -> Result<(), Error> {
        if let Some(record) = self.read(digest)? {
            return self.synchronize_existing(&record, bytes, sync);
        }
        let mut file = self.anonymous()?;
        file.write_all(bytes).map_err(|_| Error::WriteFailed)?;
        sync(&file).map_err(|_| Error::WriteFailed)?;
        self.verify()?;
        // Only this held anonymous inode is followed; linkat never replaces the destination.
        match linkat(
            CWD,
            format!("/proc/self/fd/{}", file.as_raw_fd()),
            &self.handle,
            name(digest),
            AtFlags::SYMLINK_FOLLOW,
        ) {
            Ok(()) => {
                sync(&file).map_err(|_| Error::WriteFailed)?;
                sync(&self.handle).map_err(|_| Error::WriteFailed)
            }
            Err(rustix::io::Errno::EXIST) => {
                let record = self.read(digest)?.ok_or(Error::Missing)?;
                self.synchronize_existing(&record, bytes, sync)
            }
            Err(error) => Err(storage_error(error)),
        }
    }

    fn anonymous(&self) -> Result<File, Error> {
        self.verify()?;
        let file = File::from(
            openat2(
                &self.handle,
                ".",
                OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
                CONFINED,
            )
            .map_err(storage_error)?,
        );
        let metadata = file.metadata().map_err(|_| Error::WriteFailed)?;
        if !metadata.is_file()
            || metadata.nlink() != 0
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(Error::WriteFailed);
        }
        Ok(file)
    }

    fn synchronize_existing(
        &self,
        record: &RegularFileRead,
        bytes: &[u8],
        mut sync: impl FnMut(&File) -> io::Result<()>,
    ) -> Result<(), Error> {
        if record.bytes != bytes {
            return Err(Error::Conflict);
        }
        sync(&record.file).map_err(|_| Error::WriteFailed)?;
        sync(&self.handle).map_err(|_| Error::WriteFailed)
    }
}

fn name(digest: &ArtifactDigest) -> String {
    format!("{}.hzov", digest.as_str())
}

fn storage_error(error: rustix::io::Errno) -> Error {
    match error {
        rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP => Error::Unsupported,
        _ => Error::WriteFailed,
    }
}

#[cfg(test)]
mod tests;
