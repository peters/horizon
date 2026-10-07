//! Retain file and parent identities for private tunnel staging and exact cleanup.
use crate::{Error, Result};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::ffi::OsString;

pub(super) struct OwnedFile {
    file: File,
    path: PathBuf,
    #[cfg(unix)]
    parent: File,
    #[cfg(unix)]
    name: OsString,
    cleanup_on_drop: bool,
    #[cfg(unix)]
    retired: bool,
}
impl OwnedFile {
    pub(super) fn new(suffix: &str) -> Result<Self> {
        #[cfg(unix)]
        {
            Self::from_temp(
                tempfile::Builder::new()
                    .suffix(suffix)
                    .tempfile()
                    .map_err(|_| Error::TunnelStartFailed)?,
            )
        }
        #[cfg(not(unix))]
        {
            let _ = suffix;
            Err(Error::TunnelStartFailed)
        }
    }
    #[cfg(unix)]
    fn from_temp(temp: tempfile::NamedTempFile) -> Result<Self> {
        use rustix::fs::{Mode, OFlags, open};
        let (file, mut path) = temp.into_parts();
        path.disable_cleanup(true);
        let name = path.file_name().ok_or(Error::TunnelCleanupUncertain)?.to_owned();
        let location = path
            .parent()
            .ok_or(Error::TunnelCleanupUncertain)?
            .canonicalize()
            .map_err(|_| Error::TunnelCleanupUncertain)?;
        let parent = File::from(
            open(
                &location,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::TunnelCleanupUncertain)?,
        );
        let owned = Self {
            file,
            path: location.join(&name),
            parent,
            name,
            cleanup_on_drop: true,
            retired: false,
        };
        owned.current()?;
        Ok(owned)
    }
    #[cfg(unix)]
    pub(super) fn as_file(&self) -> &File {
        &self.file
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn disable_cleanup(&mut self, disabled: bool) {
        self.cleanup_on_drop = !disabled;
    }
    // Executable staging must close its writable descriptor before exec.
    pub(super) fn seal(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            self.file = self.current()?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(Error::TunnelStartFailed)
        }
    }
    #[cfg(unix)]
    fn current(&self) -> Result<File> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::os::unix::fs::MetadataExt;
        let current = File::from(
            openat(
                &self.parent,
                &self.name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::TunnelCleanupUncertain)?,
        );
        let expected = self.file.metadata().map_err(|_| Error::TunnelCleanupUncertain)?;
        let actual = current.metadata().map_err(|_| Error::TunnelCleanupUncertain)?;
        if !actual.is_file() || (expected.dev(), expected.ino()) != (actual.dev(), actual.ino()) {
            return Err(Error::TunnelCleanupUncertain);
        }
        Ok(current)
    }
    pub(super) fn retire(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, statat, unlinkat};
            use std::os::unix::fs::MetadataExt;
            if !self.retired {
                match self.current() {
                    Ok(_current) => unlinkat(&self.parent, &self.name, AtFlags::empty())
                        .map_err(|_| Error::TunnelCleanupUncertain)?,
                    Err(error) => {
                        // Accept prior unlink only when the name is absent and the retained inode has no links.
                        // Any replacement entry keeps cleanup uncertain, even if the original was unlinked.
                        if self.file.metadata().map_err(|_| Error::TunnelCleanupUncertain)?.nlink() != 0
                            || !matches!(
                                statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW),
                                Err(rustix::io::Errno::NOENT)
                            )
                        {
                            return Err(error);
                        }
                    }
                }
                self.retired = true;
            }
            self.parent.sync_all().map_err(|_| Error::TunnelCleanupUncertain)
        }
        #[cfg(not(unix))]
        {
            Err(Error::TunnelCleanupUncertain)
        }
    }
}
impl AsRef<Path> for OwnedFile {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}
impl AsRef<OsStr> for OwnedFile {
    fn as_ref(&self) -> &OsStr {
        self.path().as_os_str()
    }
}
impl Write for OwnedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.file.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
impl Drop for OwnedFile {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = self.retire();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn replaced_path_refuses_cleanup_and_preserves_both_files() {
        let folder = tempfile::tempdir().unwrap();
        let mut file = OwnedFile::from_temp(tempfile::NamedTempFile::new_in(folder.path()).unwrap()).unwrap();
        file.disable_cleanup(true);
        file.write_all(b"synthetic credential").unwrap();
        file.flush().unwrap();
        let original = file.path().to_owned();
        let moved = folder.path().join("moved-original");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::write(&original, b"unrelated replacement").unwrap();
        assert_eq!(file.retire(), Err(Error::TunnelCleanupUncertain));
        assert_eq!(std::fs::read(&original).unwrap(), b"unrelated replacement");
        assert_eq!(std::fs::read(&moved).unwrap(), b"synthetic credential");
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&moved, &original).unwrap();
        file.retire().unwrap();
        assert!(!original.exists());
        file.retire().unwrap();
    }
    #[test]
    fn parent_replacement_cannot_redirect_owned_cleanup() {
        let folder = tempfile::tempdir().unwrap();
        let parent = folder.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let mut file = OwnedFile::from_temp(tempfile::NamedTempFile::new_in(&parent).unwrap()).unwrap();
        file.disable_cleanup(true);
        file.write_all(b"synthetic credential").unwrap();
        file.flush().unwrap();
        let name = file.name.clone();
        let moved = folder.path().join("moved-parent");
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join(&name), b"unrelated replacement").unwrap();
        file.retire().unwrap();
        assert!(!moved.join(&name).exists());
        assert_eq!(std::fs::read(parent.join(name)).unwrap(), b"unrelated replacement");
    }
    #[test]
    fn removed_original_is_idempotent_and_does_not_remove_a_later_replacement() {
        let folder = tempfile::tempdir().unwrap();
        let mut file = OwnedFile::from_temp(tempfile::NamedTempFile::new_in(folder.path()).unwrap()).unwrap();
        let path = file.path().to_owned();
        file.retire().unwrap();
        std::fs::write(&path, b"unrelated replacement").unwrap();
        file.retire().unwrap();
        drop(file);
        assert_eq!(std::fs::read(path).unwrap(), b"unrelated replacement");
    }

    #[test]
    fn unacknowledged_unlink_with_a_replacement_still_refuses_completion() {
        let folder = tempfile::tempdir().unwrap();
        let mut file = OwnedFile::from_temp(tempfile::NamedTempFile::new_in(folder.path()).unwrap()).unwrap();
        let path = file.path().to_owned();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"unrelated replacement").unwrap();
        assert_eq!(file.retire(), Err(Error::TunnelCleanupUncertain));
        assert_eq!(std::fs::read(&path).unwrap(), b"unrelated replacement");
        std::fs::remove_file(&path).unwrap();
        file.retire().unwrap();
    }
}
