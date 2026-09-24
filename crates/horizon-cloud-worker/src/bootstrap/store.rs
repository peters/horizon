//! Existing storage only. Neither a directory nor its allocation lock is created here.
use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};
pub(super) const LIMIT: u64 = 64 * 1024;
const FLAGS: OFlags = OFlags::RDONLY.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);
const LOCK: &str = "allocation.lock";
const STAGED: &str = ".recovery.next";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Publication {
    Staged,
    Renamed,
    Durable,
}

pub(super) fn invalid() -> io::Error {
    io::Error::other("Allocation bootstrap is missing, conflicting or uncertain")
}

pub(super) struct Store {
    root: PathBuf,
    directory: File,
    lock: File,
    #[cfg(test)]
    fail_sync_after: std::cell::Cell<Option<usize>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        // A concurrent fork can retain the file description until exec closes it.
        let _ = self.lock.unlock();
    }
}

impl Store {
    pub(super) fn open(root: &Path) -> io::Result<Self> {
        let directory = open_directory(root)?;
        let lock = regular(&directory, LOCK)?;
        lock.try_lock().map_err(|_| io::Error::other("Allocation is locked"))?;
        let store = Self {
            root: root.to_owned(),
            directory,
            lock,
            #[cfg(test)]
            fail_sync_after: std::cell::Cell::new(None),
        };
        store.verify()?;
        Ok(store)
    }

    pub(super) fn verify(&self) -> io::Result<()> {
        let current = open_directory(&self.root)?;
        same(&self.directory, &current)?;
        same(&self.lock, &regular(&current, LOCK)?)
    }

    pub(super) fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        self.verify()?;
        let file = match regular(&self.directory, name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            return Err(invalid());
        }
        self.verify()?;
        Ok(Some(bytes))
    }

    pub(super) fn write(&self, name: &str, expected: Option<&[u8]>, bytes: &[u8]) -> io::Result<()> {
        self.write_with(name, expected, bytes, &mut |_| Ok(()))
    }

    #[cfg(test)]
    pub(super) fn fail_sync_after(&self, successful_calls: usize) {
        self.fail_sync_after.set(Some(successful_calls));
    }

    pub(super) fn sync(&self, name: &str) -> io::Result<()> {
        #[cfg(test)]
        if let Some(remaining) = self.fail_sync_after.get() {
            if remaining == 0 {
                return Err(io::Error::other("injected synchronization failure"));
            }
            self.fail_sync_after.set(Some(remaining - 1));
        }
        self.verify()?;
        let file = regular(&self.directory, name)?;
        file.sync_all()?;
        self.directory.sync_all()?;
        same(&file, &regular(&self.directory, name)?)?;
        self.verify()
    }

    pub(super) fn write_with(
        &self,
        name: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
        checkpoint: &mut impl FnMut(Publication) -> io::Result<()>,
    ) -> io::Result<()> {
        if bytes.len() as u64 > LIMIT || self.read(name)?.as_deref() != expected {
            return Err(invalid());
        }
        // An unpublished staging file has no authority. A prior crash may leave
        // partial bytes; only a private regular file under the same lock is removed.
        if let Some(_bytes) = self.read(STAGED)? {
            unlinkat(&self.directory, STAGED, AtFlags::empty())?;
            self.directory.sync_all()?;
        }
        let mut staged = File::from(openat(
            &self.directory,
            STAGED,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        staged.write_all(bytes)?;
        staged.sync_all()?;
        checkpoint(Publication::Staged)?;
        self.verify()?;
        if self.read(name)?.as_deref() != expected {
            return Err(invalid());
        }
        same(&staged, &regular(&self.directory, STAGED)?)?;
        renameat(&self.directory, STAGED, &self.directory, name)?;
        checkpoint(Publication::Renamed)?;
        self.directory.sync_all()?;
        checkpoint(Publication::Durable)?;
        if self.read(name)?.as_deref() != Some(bytes) {
            return Err(invalid());
        }
        Ok(())
    }
}

fn same(left: &File, right: &File) -> io::Result<()> {
    let left = left.metadata()?;
    let right = right.metadata()?;
    if (left.dev(), left.ino()) != (right.dev(), right.ino()) {
        return Err(invalid());
    }
    private(&left)?;
    private(&right)
}

fn private(metadata: &std::fs::Metadata) -> io::Result<()> {
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    Ok(())
}

fn regular(directory: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(directory, name, FLAGS | OFlags::NONBLOCK, Mode::empty())?);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid());
    }
    private(&metadata)?;
    Ok(file)
}

fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut file = File::open("/")?;
    for component in path.components() {
        let name = match component {
            Component::RootDir => continue,
            Component::Normal(name) => name,
            _ => return Err(invalid()),
        };
        file = File::from(openat(&file, name, FLAGS | OFlags::DIRECTORY, Mode::empty())?);
    }
    private(&file.metadata()?)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn dropping_store_releases_lock_even_when_another_descriptor_remains_open() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join(LOCK);
        std::fs::write(&path, b"existing lock").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let store = Store::open(root.path()).unwrap();
        let inherited = store.lock.try_clone().unwrap();
        assert!(Store::open(root.path()).is_err());
        drop(store);
        let reopened = Store::open(root.path()).unwrap();
        assert!(Store::open(root.path()).is_err());
        drop(inherited);
        assert!(Store::open(root.path()).is_err());
        drop(reopened);
        assert!(Store::open(root.path()).is_ok());
    }
}
