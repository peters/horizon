use super::PrivateCheckoutError as Error;
use crate::repository_overlay::reader::SelectedRepositoryReader;
use git2::{ObjectType, Oid};
use rustix::fs::{Mode, OFlags, ResolveFlags, mkdirat, openat2, readlinkat_raw, symlinkat};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(super) struct Root(File);

impl Root {
    pub(super) fn handle(&self) -> &File {
        &self.0
    }

    pub(super) fn open(path: &Path) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(path).map_err(|_| Error::UnsafeNode)?;
        let root = Self(reader.root.handle().try_clone().map_err(|_| Error::Storage)?);
        root.verify(path)?;
        let mut entries = fs::read_dir(pinned(&root.0)).map_err(|_| Error::Storage)?;
        let only = entries.next().ok_or(Error::UnsafeNode)?.map_err(|_| Error::Storage)?;
        if only.file_name() != ".git" || entries.next().is_some() {
            return Err(Error::UnsafeNode);
        }
        root.parent(".git")?;
        Ok(root)
    }

    pub(super) fn verify(&self, path: &Path) -> Result<(), Error> {
        let current = SelectedRepositoryReader::open(path).map_err(|_| Error::UnsafeNode)?;
        let actual = current.root.handle().metadata().map_err(|_| Error::Storage)?;
        let held = self.0.metadata().map_err(|_| Error::Storage)?;
        if !actual.is_dir()
            || actual.uid() != rustix::process::geteuid().as_raw()
            || actual.mode() & 0o7777 != 0o700
            || actual.nlink() == 0
            || (actual.dev(), actual.ino()) != (held.dev(), held.ino())
        {
            return Err(Error::UnsafeNode);
        }
        Ok(())
    }

    fn parent(&self, path: &str) -> Result<File, Error> {
        let fd = openat2(
            &self.0,
            path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        )
        .map_err(|_| Error::UnsafeNode)?;
        Ok(File::from(fd))
    }

    fn split<'a>(&self, path: &'a str) -> Result<(File, &'a str), Error> {
        let (parent, leaf) = path.rsplit_once('/').unwrap_or((".", path));
        Ok((self.parent(parent)?, leaf))
    }

    pub(super) fn directory(&self, path: &str) -> Result<(), Error> {
        let (parent, leaf) = self.split(path)?;
        mkdirat(parent, leaf, Mode::RUSR | Mode::WUSR | Mode::XUSR).map_err(|_| Error::UnsafeNode)?;
        let directory = self.parent(path)?;
        let metadata = directory.metadata().map_err(|_| Error::Storage)?;
        if metadata.mode() & 0o7777 != 0o700 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::UnsafeNode);
        }
        Ok(())
    }

    pub(super) fn create(&self, path: &str) -> Result<File, Error> {
        let (parent, leaf) = self.split(path)?;
        let fd = openat2(
            parent,
            leaf,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            CONFINED,
        )
        .map_err(|_| Error::UnsafeNode)?;
        Ok(File::from(fd))
    }

    pub(super) fn read_regular(&self, path: &str) -> Result<File, Error> {
        let file = self.pin(path)?;
        let before = file.metadata().map_err(|_| Error::Storage)?;
        regular(&before)?;
        let input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(pinned(&file))
            .map_err(|_| Error::Storage)?;
        if !same(&before, &input.metadata().map_err(|_| Error::Storage)?) {
            return Err(Error::UnsafeNode);
        }
        Ok(input)
    }

    fn pin(&self, path: &str) -> Result<File, Error> {
        openat2(
            &self.0,
            path,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        )
        .map(File::from)
        .map_err(|_| Error::UnsafeNode)
    }

    pub(super) fn verify_file(
        &self,
        path: &str,
        file: &File,
        bytes: u64,
        executable: bool,
        id: Oid,
    ) -> Result<(), Error> {
        let mode = if executable { 0o755 } else { 0o644 };
        regular(&file.metadata().map_err(|_| Error::Storage)?)?;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|_| Error::Storage)?;
        let before = file.metadata().map_err(|_| Error::Storage)?;
        if before.len() != bytes || before.mode() & 0o7777 != mode {
            return Err(Error::Object);
        }
        if Oid::hash_file(ObjectType::Blob, Path::new(&pinned(file))).map_err(|_| Error::Object)? != id {
            return Err(Error::Object);
        }
        if !same(&before, &file.metadata().map_err(|_| Error::Storage)?)
            || !same(&before, &self.pin(path)?.metadata().map_err(|_| Error::Storage)?)
        {
            return Err(Error::UnsafeNode);
        }
        Ok(())
    }

    pub(super) fn symlink(&self, path: &str, target: &str) -> Result<(), Error> {
        let (parent, leaf) = self.split(path)?;
        symlinkat(target, parent, leaf).map_err(|_| Error::UnsafeNode)?;
        let link = self.pin(path)?;
        let metadata = link.metadata().map_err(|_| Error::Storage)?;
        if !metadata.is_symlink() || metadata.nlink() != 1 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::UnsafeNode);
        }
        let mut buffer = [0; 4097];
        let length = readlinkat_raw(link, "", &mut buffer[..]).map_err(|_| Error::Storage)?;
        if buffer[..length] != *target.as_bytes() {
            return Err(Error::Object);
        }
        Ok(())
    }
}

fn pinned(file: &File) -> String {
    format!("/proc/self/fd/{}", file.as_raw_fd())
}

pub(super) fn regular(metadata: &Metadata) -> Result<(), Error> {
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != rustix::process::geteuid().as_raw() {
        Err(Error::UnsafeNode)
    } else {
        Ok(())
    }
}

pub(super) fn same(a: &Metadata, b: &Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.mode(),
        a.uid(),
        a.nlink(),
        a.len(),
        a.mtime(),
        a.mtime_nsec(),
        a.ctime(),
        a.ctime_nsec(),
    ) == (
        b.dev(),
        b.ino(),
        b.mode(),
        b.uid(),
        b.nlink(),
        b.len(),
        b.mtime(),
        b.mtime_nsec(),
        b.ctime(),
        b.ctime_nsec(),
    )
}
