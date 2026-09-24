//! Artifact I/O stays relative to one opened canonical directory.
use super::{Error, Result};
#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
#[cfg(unix)]
use std::io::{Read, Write};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 8 * 1024 * 1024;
pub(super) struct Directory {
    path: PathBuf,
    file: File,
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
        let directory = Self {
            path: path.to_owned(),
            file: Self::open_path(path)?,
        };
        directory.verify()?;
        Ok(directory)
    }
    fn verify(&self) -> Result<()> {
        self.verify_with(|| {})
    }
    pub(super) fn verify_with(&self, checkpoint: impl FnOnce()) -> Result<()> {
        let opened = self.file.metadata()?;
        checkpoint();
        let current = Self::open_path(&self.path)?.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (current.dev(), current.ino()) != (opened.dev(), opened.ino()) {
                return Err(Error::Ownership);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (opened, current);
            Err(Error::Ownership)
        }
    }
    fn open_path(path: &Path) -> Result<File> {
        if !path.is_absolute() {
            return Err(Error::Ownership);
        }
        #[cfg(unix)]
        {
            use std::path::Component;
            let mut directory = File::open("/")?;
            for component in path.components() {
                let name = match component {
                    Component::RootDir => continue,
                    Component::Normal(name) => name,
                    _ => return Err(Error::Ownership),
                };
                directory = File::from(
                    openat(
                        &directory,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?,
                );
            }
            Ok(directory)
        }
        #[cfg(not(unix))]
        Err(Error::Ownership)
    }
    pub(super) fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.verify()?;
        #[cfg(unix)]
        {
            let fd = openat(
                &self.file,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
            let file = File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(Error::Journal);
            }
            let mut bytes = Vec::new();
            file.take(LIMIT + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > LIMIT {
                return Err(Error::Journal);
            }
            self.verify()?;
            Ok(bytes)
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(Error::Ownership)
        }
    }
    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.verify()?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Journal);
        }
        #[cfg(unix)]
        {
            let staged = format!(".{}.tmp", uuid::Uuid::new_v4());
            let fd = openat(
                &self.file,
                &staged,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(std::io::Error::from)?;
            let result = (|| {
                let mut file = File::from(fd);
                file.write_all(bytes)?;
                file.sync_all()?;
                renameat(&self.file, &staged, &self.file, name).map_err(std::io::Error::from)?;
                self.file.sync_all()?;
                self.verify()
            })();
            let _ = unlinkat(&self.file, &staged, AtFlags::empty());
            result
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(Error::Ownership)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn retained_directory_is_not_inherited_by_executed_children() {
        let temp = tempfile::tempdir().unwrap();
        let directory = Directory::open(&temp.path().canonicalize().unwrap()).unwrap();
        assert!(
            rustix::io::fcntl_getfd(&directory.file)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
    }
}
