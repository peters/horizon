//! Held-file verification, not immutable storage or hostile-writer confinement.
use super::{ControllerError as Error, EncodedIdentity};
use crate::repository_overlay::reader::{
    SelectedRepositoryReader,
    linux::{Root, same_metadata},
};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub(super) struct Input<'a> {
    pub file: File,
    path: &'a Path,
    parent: Root,
    metadata: Metadata,
}

impl<'a> Input<'a> {
    pub fn open(path: &'a Path, expected: &EncodedIdentity, cancelled: &dyn Fn() -> bool) -> Result<Self, Error> {
        let parent = SelectedRepositoryReader::open(path.parent().ok_or(Error::Input)?)
            .map_err(|_| Error::Input)?
            .root;
        private_parent(parent.handle())?;
        let pinned = pin(&parent, path)?;
        let metadata = pinned.metadata().map_err(|_| Error::Input)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.nlink() != 1
            || metadata.len() != expected.encoded_bytes
        {
            return Err(Error::Input);
        }
        // Reopen the already pinned regular inode, never a replacement device/FIFO.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
            .map_err(|_| Error::Input)?;
        let mut input = Self {
            file,
            path,
            parent,
            metadata,
        };
        input.verify()?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 16 * 1024];
        let mut total = 0;
        loop {
            super::check_cancel(cancelled)?;
            let count = input.file.read(&mut buffer).map_err(|_| Error::Input)?;
            total += count as u64;
            if total > expected.encoded_bytes {
                return Err(Error::Input);
            }
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if total != expected.encoded_bytes
            || crate::cloud_run::ArtifactDigest::from_sha256_bytes(hash.finalize().into()) != expected.sha256
        {
            return Err(Error::Input);
        }
        input.verify()?;
        input.file.rewind().map_err(|_| Error::Input)?;
        super::check_cancel(cancelled)?;
        Ok(input)
    }

    pub fn verify(&self) -> Result<(), Error> {
        let named =
            SelectedRepositoryReader::open(self.path.parent().ok_or(Error::Input)?).map_err(|_| Error::Input)?;
        private_parent(named.root.handle())?;
        let parent = self.parent.handle().metadata().map_err(|_| Error::Input)?;
        let current = named.root.handle().metadata().map_err(|_| Error::Input)?;
        let named_file = pin(&named.root, self.path)?.metadata().map_err(|_| Error::Input)?;
        if (parent.dev(), parent.ino()) != (current.dev(), current.ino())
            || !same_metadata(&self.metadata, &self.file.metadata().map_err(|_| Error::Input)?)
            || !same_metadata(&self.metadata, &named_file)
        {
            return Err(Error::Input);
        }
        Ok(())
    }
}

fn pin(parent: &Root, path: &Path) -> Result<File, Error> {
    openat2(
        parent.handle(),
        path.file_name().ok_or(Error::Input)?,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_XDEV,
    )
    .map(File::from)
    .map_err(|_| Error::Input)
}

fn private_parent(file: &File) -> Result<(), Error> {
    let metadata = file.metadata().map_err(|_| Error::Input)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
        || metadata.nlink() == 0
    {
        return Err(Error::Input);
    }
    Ok(())
}
