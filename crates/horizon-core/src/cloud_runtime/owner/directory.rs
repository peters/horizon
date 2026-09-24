//! Artifact I/O stays relative to one opened canonical directory.
use super::{Error, Result};
#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
#[cfg(unix)]
use std::io::{Read, Write};
use std::{
    fs::{self, File, OpenOptions},
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
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let directory = Self {
            path: path.to_owned(),
            file: options.open(path)?,
        };
        directory.verify()?;
        Ok(directory)
    }
    fn verify(&self) -> Result<()> {
        let path = fs::symlink_metadata(&self.path)?;
        let opened = self.file.metadata()?;
        if !path.is_dir() || !opened.is_dir() || self.path.canonicalize()? != self.path {
            return Err(Error::Ownership);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (path.dev(), path.ino()) != (opened.dev(), opened.ino()) {
                return Err(Error::Ownership);
            }
            Ok(())
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
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
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
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
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
