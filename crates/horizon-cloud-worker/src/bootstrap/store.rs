//! Descriptor-anchored allocation storage with exclusive first publication.
use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags, mkdirat, openat, renameat, renameat_with, unlinkat};
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
    pub(super) fn require_pristine(workspace: &Path) -> io::Result<()> {
        pristine(&anchor_directory(workspace)?, None)
    }

    /// Called only after independent runtime and signed first-create checks.
    /// Existing or partial roots are never adopted by this constructor.
    pub(super) fn create(root: &Path) -> io::Result<Self> {
        let workspace = root.parent().ok_or_else(invalid)?;
        let name = root.file_name().ok_or_else(invalid)?;
        let parent = anchor_directory(workspace)?;
        pristine(&parent, None)?;
        let staged = format!(".horizon-bootstrap-{}", horizon_cloud_protocol::OperationId::generate());
        mkdirat(&parent, staged.as_str(), Mode::RUSR | Mode::WUSR | Mode::XUSR)?;
        let directory = File::from(openat(
            &parent,
            staged.as_str(),
            FLAGS | OFlags::DIRECTORY,
            Mode::empty(),
        )?);
        private(&directory.metadata()?)?;
        let lock = File::from(openat(
            &directory,
            LOCK,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        lock.try_lock().map_err(|_| invalid())?;
        lock.sync_all()?;
        directory.sync_all()?;
        pristine(&parent, Some(staged.as_bytes()))?;
        renameat_with(&parent, staged.as_str(), &parent, name, RenameFlags::NOREPLACE)?;
        parent.sync_all()?;
        let store = Self {
            root: root.to_owned(),
            directory,
            lock,
            #[cfg(test)]
            fail_sync_after: std::cell::Cell::new(None),
        };
        store.verify()?;
        pristine(&parent, Some(name.as_encoded_bytes()))?;
        Ok(store)
    }

    pub(super) fn pristine_workspace(&self) -> io::Result<()> {
        self.verify()?;
        let parent = self.root.parent().ok_or_else(invalid)?;
        let directory = anchor_directory(parent)?;
        pristine(
            &directory,
            Some(self.root.file_name().ok_or_else(invalid)?.as_encoded_bytes()),
        )?;
        self.verify()
    }

    pub(super) fn create_host_key(&self, bytes: &[u8]) -> io::Result<()> {
        self.verify()?;
        if bytes.is_empty() || bytes.len() > 8192 {
            return Err(invalid());
        }
        let mut file = File::from(openat(
            &self.directory,
            "ssh-host-key",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        file.write_all(bytes)?;
        file.sync_all()?;
        self.directory.sync_all()?;
        same(&file, &regular(&self.directory, "ssh-host-key")?)?;
        self.verify()
    }

    pub(super) fn host_key(&self) -> io::Result<zeroize::Zeroizing<Vec<u8>>> {
        self.verify()?;
        let mut file = regular(&self.directory, "ssh-host-key")?;
        let bytes = secret(&mut file)?;
        same(&file, &regular(&self.directory, "ssh-host-key")?)?;
        self.verify()?;
        Ok(bytes)
    }
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

pub(super) fn secret(file: &mut File) -> io::Result<zeroize::Zeroizing<Vec<u8>>> {
    let mut bytes = zeroize::Zeroizing::new(vec![0; 8193]);
    let mut length = 0;
    while length < bytes.len() {
        let read = file.read(&mut bytes[length..])?;
        if read == 0 {
            break;
        }
        length += read;
    }
    if length == 0 || length > 8192 {
        return Err(invalid());
    }
    bytes.truncate(length);
    Ok(bytes)
}

fn pristine(directory: &File, allowed: Option<&[u8]>) -> io::Result<()> {
    for entry in rustix::fs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." && Some(name) != allowed {
            return Err(invalid());
        }
    }
    Ok(())
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

pub(super) fn regular(directory: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(directory, name, FLAGS | OFlags::NONBLOCK, Mode::empty())?);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid());
    }
    private(&metadata)?;
    Ok(file)
}

pub(super) fn open_directory(path: &Path) -> io::Result<File> {
    let file = anchor_directory(path)?;
    private(&file.metadata()?)?;
    Ok(file)
}

pub(super) fn anchor_directory(path: &Path) -> io::Result<File> {
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
