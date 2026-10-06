use crate::{Error, Result};
use std::path::Path;

#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::{
    fs::File,
    io::{Read, Write},
};

#[cfg(unix)]
pub struct Directory {
    file: File,
}

#[cfg(unix)]
impl Directory {
    /// # Errors
    /// Requires an existing private, owned directory without symlink components.
    pub fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        if !path.is_absolute() {
            return Err(Error::StateUnavailable);
        }
        let mut file = File::open("/").map_err(|_| Error::StateUnavailable)?;
        for component in path.components().skip(1) {
            let std::path::Component::Normal(name) = component else {
                return Err(Error::StateUnavailable);
            };
            file = File::from(
                rustix::fs::openat(
                    &file,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::StateUnavailable)?,
            );
        }
        let metadata = file.metadata().map_err(|_| Error::StateUnavailable)?;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(Error::StateUnavailable);
        }
        Ok(Self { file })
    }

    /// # Errors
    /// Refuses existing names, symlinks and unavailable private storage.
    pub fn new_file(&self, name: &str) -> Result<File> {
        let path = Path::new(name);
        if name.is_empty()
            || name.len() > 255
            || name.chars().any(char::is_control)
            || path.components().count() != 1
            || !matches!(path.components().next(), Some(std::path::Component::Normal(_)))
            || path.file_name() != Some(std::ffi::OsStr::new(name))
        {
            return Err(Error::StateUnavailable);
        }
        let file = File::from(
            rustix::fs::openat(
                &self.file,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        self.file.sync_all().map_err(|_| Error::StateUnavailable)?;
        Ok(file)
    }

    /// # Errors
    /// Create a fresh owned operation directory beneath the held private root, without path traversal.
    pub fn create_child(&self, name: &str) -> Result<()> {
        self.create_child_tracked(name, || ())
    }

    /// # Errors
    /// Register ownership immediately after mkdir, including when the following parent sync fails.
    /// Existing or foreign children never invoke the ownership callback.
    pub fn create_child_tracked(&self, name: &str, created: impl FnOnce()) -> Result<()> {
        self.create_child_with_sync(name, created, || self.file.sync_all())
    }

    fn create_child_with_sync(
        &self,
        name: &str,
        created: impl FnOnce(),
        sync: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<()> {
        operation_name(name)?;
        rustix::fs::mkdirat(&self.file, name, Mode::RWXU).map_err(|_| Error::StateUnavailable)?;
        created();
        sync().map_err(|_| Error::StateUnavailable)
    }

    /// # Errors
    /// Open one owned child using the held root capability, never a reopened absolute path.
    pub fn child(&self, name: &str) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        operation_name(name)?;
        let file = File::from(
            rustix::fs::openat(
                &self.file,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        let metadata = file.metadata().map_err(|_| Error::StateUnavailable)?;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(Error::StateUnavailable);
        }
        Ok(Self { file })
    }

    /// # Errors
    /// Bounded host startup observation through the held directory; never reopens an export path.
    pub fn is_empty(&self) -> Result<bool> {
        let entries = rustix::fs::Dir::read_from(&self.file).map_err(|_| Error::StateUnavailable)?;
        for entry in entries {
            let entry = entry.map_err(|_| Error::StateUnavailable)?;
            if !matches!(entry.file_name().to_bytes(), b"." | b"..") {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// # Errors
    /// Count only private UUID child directories through this retained capability, stopping at the cap.
    pub fn count_children(&self, cap: usize) -> Result<usize> {
        let entries = rustix::fs::Dir::read_from(&self.file).map_err(|_| Error::StateUnavailable)?;
        let mut count = 0;
        for entry in entries {
            let entry = entry.map_err(|_| Error::StateUnavailable)?;
            let bytes = entry.file_name().to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }
            let name = std::str::from_utf8(bytes).map_err(|_| Error::StateUnavailable)?;
            self.child(name)?;
            count += 1;
            if count >= cap {
                return Ok(count);
            }
        }
        Ok(count)
    }

    /// # Errors
    /// Before exporting a path, verify it still resolves to the held private directory.
    pub fn matches_path(&self, path: &Path) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let current = Self::open(path)?;
        let expected = self.file.metadata().map_err(|_| Error::StateUnavailable)?;
        let actual = current.file.metadata().map_err(|_| Error::StateUnavailable)?;
        if (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
            return Err(Error::StateUnavailable);
        }
        Ok(())
    }
    /// # Errors
    /// Trusted host use only, after durable operation completion. Anchored cleanup never follows links.
    /// Missing directories are idempotent after cleanup; incomplete operations must never call this API.
    pub fn retire_child(&self, name: &str) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        operation_name(name)?;
        let child = match rustix::fs::openat(
            &self.file,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return self.file.sync_all().map_err(|_| Error::StateUnavailable),
            Err(_) => return Err(Error::StateUnavailable),
        };
        let expected = child.metadata().map_err(|_| Error::StateUnavailable)?;
        if expected.uid() != rustix::process::getuid().as_raw() || expected.mode() & 0o077 != 0 {
            return Err(Error::StateUnavailable);
        }
        clear_completed(&child, 0, &mut 512)?;
        let current =
            rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Error::StateUnavailable)?;
        if (identity_number(current.st_dev)?, identity_number(current.st_ino)?) != (expected.dev(), expected.ino()) {
            return Err(Error::StateUnavailable);
        }
        rustix::fs::unlinkat(&self.file, name, AtFlags::REMOVEDIR).map_err(|_| Error::StateUnavailable)?;
        self.file.sync_all().map_err(|_| Error::StateUnavailable)
    }

    /// # Errors
    /// Only an exact journal-proven undispatched operation may retire an empty child.
    /// Missing children are safe; nonempty directories and replacements remain held.
    pub fn retire_empty_child(&self, name: &str) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        operation_name(name)?;
        match rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => return self.file.sync_all().map_err(|_| Error::StateUnavailable),
            Err(_) => return Err(Error::StateUnavailable),
            Ok(_) => (),
        }
        let child = self.child(name)?;
        if !child.is_empty()? {
            return Err(Error::StateUnavailable);
        }
        let expected = child.file.metadata().map_err(|_| Error::StateUnavailable)?;
        let current =
            rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| Error::StateUnavailable)?;
        if (identity_number(current.st_dev)?, identity_number(current.st_ino)?) != (expected.dev(), expected.ino()) {
            return Err(Error::StateUnavailable);
        }
        // rmdir cannot remove an unexpected file created after the emptiness observation.
        rustix::fs::unlinkat(&self.file, name, AtFlags::REMOVEDIR).map_err(|_| Error::StateUnavailable)?;
        self.file.sync_all().map_err(|_| Error::StateUnavailable)
    }

    /// # Errors
    /// Host-only bounded receipt read. Rejects links, special files, shared permissions and foreign owners.
    pub fn receipt<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        use std::os::unix::fs::MetadataExt;
        let file = File::from(
            rustix::fs::openat(
                &self.file,
                "process.json",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        let metadata = file.metadata().map_err(|_| Error::StateUnavailable)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(Error::StateUnavailable);
        }
        let mut bytes = Vec::new();
        file.take(32769)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::StateUnavailable)?;
        if bytes.len() > 32768 {
            return Err(Error::StateUnavailable);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::StateUnavailable)
    }

    /// # Errors
    /// A failed private atomic write retains the prior receipt and returns a typed failure.
    pub fn save(&self, value: &impl serde::Serialize) -> Result<()> {
        let name = format!("process-{}.partial", uuid::Uuid::new_v4());
        let cleanup = Partial {
            directory: &self.file,
            name: &name,
        };
        let mut file = self.new_file(&name)?;
        serde_json::to_writer(&mut file, value).map_err(|_| Error::StateUnavailable)?;
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::StateUnavailable)?;
        rustix::fs::renameat(&self.file, &name, &self.file, "process.json").map_err(|_| Error::StateUnavailable)?;
        self.file.sync_all().map_err(|_| Error::StateUnavailable)?;
        drop(cleanup);
        Ok(())
    }
}

#[cfg(unix)]
fn identity_number(value: impl TryInto<u64>) -> Result<u64> {
    value.try_into().map_err(|_| Error::StateUnavailable)
}
#[cfg(unix)]
fn operation_name(name: &str) -> Result<()> {
    let operation = uuid::Uuid::parse_str(name).map_err(|_| Error::StateUnavailable)?;
    if operation.is_nil() || operation.simple().to_string() != name {
        return Err(Error::StateUnavailable);
    }
    Ok(())
}
#[cfg(unix)]
fn clear_completed(directory: &File, depth: u8, remaining: &mut usize) -> Result<()> {
    if depth >= 8 {
        return Err(Error::StateUnavailable);
    }
    let entries = rustix::fs::Dir::read_from(directory).map_err(|_| Error::StateUnavailable)?;
    for entry in entries {
        let entry = entry.map_err(|_| Error::StateUnavailable)?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        *remaining = remaining.checked_sub(1).ok_or(Error::StateUnavailable)?;
        let flags = if entry.file_type() == rustix::fs::FileType::Directory {
            let child = File::from(
                rustix::fs::openat(
                    directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::StateUnavailable)?,
            );
            clear_completed(&child, depth + 1, remaining)?;
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        };
        rustix::fs::unlinkat(directory, name, flags).map_err(|_| Error::StateUnavailable)?;
    }
    directory.sync_all().map_err(|_| Error::StateUnavailable)
}

#[cfg(unix)]
struct Partial<'a> {
    directory: &'a File,
    name: &'a str,
}
#[cfg(unix)]
impl Drop for Partial<'_> {
    fn drop(&mut self) {
        let _ = rustix::fs::unlinkat(self.directory, self.name, AtFlags::empty());
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        Directory::open(path).map(|_| ())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::StateUnavailable)
    }
}

#[cfg(not(unix))]
pub struct Directory;
#[cfg(not(unix))]
impl Directory {
    /// # Errors
    /// Private native storage is unavailable on unsupported hosts.
    pub fn count_children(&self, _cap: usize) -> Result<usize> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Private native storage is unavailable on unsupported hosts.
    pub fn is_empty(&self) -> Result<bool> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot create tracked child storage.
    pub fn create_child_tracked(&self, _name: &str, _created: impl FnOnce()) -> Result<()> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot open anchored child storage.
    pub fn child(&self, _name: &str) -> Result<Self> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot verify export paths.
    pub fn matches_path(&self, _path: &Path) -> Result<()> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot create operation storage.
    pub fn create_child(&self, _name: &str) -> Result<()> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot retire operation storage.
    pub fn retire_child(&self, _name: &str) -> Result<()> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot retire undispatched operation storage.
    pub fn retire_empty_child(&self, _name: &str) -> Result<()> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Unsupported hosts cannot read process cleanup evidence.
    pub fn receipt<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Requires an existing private, owned directory without symlink components.
    pub fn open(_path: &Path) -> Result<Self> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// Refuses existing names, symlinks and unavailable private storage.
    pub fn new_file(&self, _name: &str) -> Result<std::fs::File> {
        Err(Error::StateUnavailable)
    }
    /// # Errors
    /// A failed private atomic write retains the prior receipt and returns a typed failure.
    pub fn save(&self, _value: &impl serde::Serialize) -> Result<()> {
        Err(Error::StateUnavailable)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn created_child_ownership_survives_sync_failure_but_never_adopts_existing_names() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let directory = Directory::open(&root.path().canonicalize().unwrap()).unwrap();
        let name = uuid::Uuid::new_v4().simple().to_string();
        let mut retained = false;
        assert_eq!(
            directory.create_child_with_sync(
                &name,
                || retained = true,
                || Err(std::io::ErrorKind::StorageFull.into())
            ),
            Err(Error::StateUnavailable)
        );
        assert!(retained);
        assert!(root.path().join(&name).is_dir());
        directory.retire_child(&name).unwrap();
        directory.create_child(&name).unwrap();
        assert_eq!(
            directory.create_child_with_sync(&name, || panic!("existing child was adopted"), || Ok(())),
            Err(Error::StateUnavailable)
        );
    }

    #[test]
    fn public_file_creation_cannot_escape_its_anchored_directory() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = Directory::open(&root).unwrap();
        let outside = root.parent().unwrap().join(format!("escape-{}", uuid::Uuid::new_v4()));
        for name in [
            "",
            ".",
            "..",
            "../outside",
            "nested/file",
            "./file",
            "file/",
            "line\nfeed",
        ] {
            assert_eq!(directory.new_file(name).err(), Some(Error::StateUnavailable));
        }
        assert_eq!(
            directory.new_file(outside.to_str().unwrap()).err(),
            Some(Error::StateUnavailable)
        );
        assert!(!outside.exists());
        directory.new_file("allowed.log").unwrap();
        assert!(root.join("allowed.log").is_file());
        assert!(directory.new_file("allowed.log").is_err());
    }
}
