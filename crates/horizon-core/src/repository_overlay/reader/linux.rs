use super::{RepositoryReadError as Error, SelectedRepositoryNode as Node, paths};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2, readlinkat_raw};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

pub(super) struct Root(File);

impl Root {
    pub(super) fn open(path: &Path) -> Result<Self, Error> {
        let descriptor = openat2(
            CWD,
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(open_error)?;
        Ok(Self(File::from(descriptor)))
    }

    fn pin(&self, path: &str) -> Result<PinnedNode, Error> {
        let descriptor = openat2(
            &self.0,
            path,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(open_error)?;
        let file = File::from(descriptor);
        let metadata = file.metadata().map_err(|_| Error::ReadFailed)?;
        if !(metadata.is_file() || metadata.is_symlink()) || metadata.nlink() != 1 {
            return Err(Error::UnsupportedNode);
        }
        Ok(PinnedNode { file, metadata })
    }

    pub(super) fn read(&self, path: &str, limit: usize) -> Result<Node, Error> {
        let node = self.pin(path)?;
        let content = if node.metadata.is_symlink() {
            node.link(path, limit)?
        } else {
            node.regular(limit)?
        };
        node.verify(&node.file.metadata().map_err(|_| Error::ReadFailed)?)?;
        self.verify_path(path, &node)?;
        Ok(content)
    }

    fn verify_path(&self, path: &str, node: &PinnedNode) -> Result<(), Error> {
        let current = self.pin(path).map_err(|_| Error::Changed)?;
        node.verify(&current.metadata)
    }
}

struct PinnedNode {
    file: File,
    metadata: Metadata,
}

impl PinnedNode {
    fn verify(&self, current: &Metadata) -> Result<(), Error> {
        if fingerprint(&self.metadata) == fingerprint(current) {
            Ok(())
        } else {
            Err(Error::Changed)
        }
    }

    fn regular(&self, limit: usize) -> Result<Node, Error> {
        let declared = usize::try_from(self.metadata.len()).map_err(|_| Error::TooLarge)?;
        if declared > limit {
            return Err(Error::TooLarge);
        }
        // The held O_PATH handle already identifies a regular inode. Reopen that
        // inode, not its mutable name, so a path swap cannot open a device or FIFO.
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
            .map_err(|_| Error::ReadFailed)?;
        self.verify(&file.metadata().map_err(|_| Error::ReadFailed)?)?;
        let bytes = read_limited(&mut file, declared, limit)?;
        self.verify(&file.metadata().map_err(|_| Error::ReadFailed)?)?;
        Ok(Node::File {
            bytes,
            executable: self.metadata.mode() & 0o100 != 0,
        })
    }

    fn link(&self, path: &str, limit: usize) -> Result<Node, Error> {
        let mut buffer = [0u8; paths::MAX_PATH_BYTES + 1];
        let length = readlinkat_raw(&self.file, "", &mut buffer[..]).map_err(|_| Error::ReadFailed)?;
        if length == buffer.len() || length > limit {
            return Err(Error::TooLarge);
        }
        let target = std::str::from_utf8(&buffer[..length])
            .map_err(|_| Error::UnsafePath)?
            .to_owned();
        paths::validate_link(path, &target)?;
        Ok(Node::Symlink { target })
    }
}

fn read_limited(reader: impl Read, declared: usize, limit: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(declared + 1).map_err(|_| Error::TooLarge)?;
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::ReadFailed)?;
    if bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    if bytes.len() != declared {
        return Err(Error::Changed);
    }
    Ok(bytes)
}

#[derive(Eq, PartialEq)]
struct Fingerprint {
    device: u64,
    inode: u64,
    bytes: u64,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn fingerprint(metadata: &Metadata) -> Fingerprint {
    Fingerprint {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn open_error(error: rustix::io::Errno) -> Error {
    match error {
        rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL => Error::Unsupported,
        rustix::io::Errno::NOENT => Error::Missing,
        rustix::io::Errno::LOOP | rustix::io::Errno::XDEV | rustix::io::Errno::AGAIN => Error::UnsafePath,
        _ => Error::ReadFailed,
    }
}

#[cfg(test)]
mod tests;
