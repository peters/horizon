//! Artifact I/O stays relative to one opened canonical directory.
use super::{Error, Result};
#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fs::{RenameFlags, mkdirat, renameat_with};
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
    pub(super) fn create(path: &Path, checkpoint: impl FnOnce()) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
        if !path.is_absolute() {
            return Err(Error::Ownership);
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let name = path.file_name().ok_or(Error::Ownership)?;
            let parent = Self::open(&path.parent().ok_or(Error::Ownership)?.canonicalize()?)?;
            let staged = format!(".{}.new", uuid::Uuid::new_v4());
            mkdirat(&parent.file, &staged, Mode::RWXU).map_err(std::io::Error::from)?;
            let file = File::from(
                openat(
                    &parent.file,
                    &staged,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            );
            let mut directory = Self {
                path: parent.path.join(&staged),
                file,
            };
            let result = (|| {
                directory.verify()?;
                directory.file.sync_all()?;
                parent.file.sync_all()?;
                checkpoint();
                parent.verify()?;
                directory.verify()?;
                renameat_with(&parent.file, &staged, &parent.file, name, RenameFlags::NOREPLACE)
                    .map_err(std::io::Error::from)?;
                directory.path = parent.path.join(name);
                parent.file.sync_all()?;
                directory.verify()
            })();
            // Leave an uncertain staging identity intact; never remove a replacement.
            if result.is_err() && directory.path == parent.path.join(&staged) && directory.verify().is_ok() {
                let _ = unlinkat(&parent.file, &staged, AtFlags::REMOVEDIR);
            }
            result?;
            Ok(directory)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = checkpoint;
            Err(Error::Ownership)
        }
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
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
        let directory = Directory::create(&temp.path().join("allocation"), || {}).unwrap();
        assert!(
            rustix::io::fcntl_getfd(&directory.file)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
    }

    #[test]
    fn creation_never_adopts_an_existing_or_competing_destination() {
        for late in [false, true] {
            for kind in ["empty", "populated", "symlink"] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("allocation");
                let external = temp.path().join("external");
                std::fs::create_dir(&external).unwrap();
                std::fs::write(external.join("sentinel"), b"original").unwrap();
                let seed = || {
                    if kind == "symlink" {
                        std::os::unix::fs::symlink(&external, &root).unwrap();
                    } else {
                        std::fs::create_dir(&root).unwrap();
                        if kind == "populated" {
                            std::fs::write(root.join("sentinel"), b"original").unwrap();
                        }
                    }
                };
                if !late {
                    seed();
                }
                assert!(
                    Directory::create(&root, || if late {
                        seed();
                    })
                    .is_err()
                );
                if kind == "empty" {
                    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
                } else {
                    assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"original");
                }
            }
        }
    }

    #[test]
    fn creation_rejects_parent_or_staging_substitution_before_publication() {
        for (parent_replaced, populated) in [(false, false), (false, true), (true, false), (true, true)] {
            let temp = tempfile::tempdir().unwrap();
            let parent = temp.path().join("parent");
            std::fs::create_dir(&parent).unwrap();
            let root = parent.join("allocation");
            assert!(
                Directory::create(&root, || {
                    let replaced = if parent_replaced {
                        parent.clone()
                    } else {
                        std::fs::read_dir(&parent).unwrap().next().unwrap().unwrap().path()
                    };
                    std::fs::rename(&replaced, temp.path().join("moved")).unwrap();
                    std::fs::create_dir(&replaced).unwrap();
                    if populated {
                        std::fs::write(replaced.join("sentinel"), b"replacement").unwrap();
                    }
                })
                .is_err()
            );
            assert!(!root.exists());
            let replacement = if parent_replaced {
                parent
            } else {
                std::fs::read_dir(parent).unwrap().next().unwrap().unwrap().path()
            };
            if populated {
                assert_eq!(std::fs::read(replacement.join("sentinel")).unwrap(), b"replacement");
            } else {
                assert_eq!(std::fs::read_dir(replacement).unwrap().count(), 0);
            }
        }
    }
}
