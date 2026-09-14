use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use atomicwrites::{AllowOverwrite, AtomicFile};
use uuid::Uuid;

use crate::RoutineError;
use crate::definition::RoutineDefinition;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};

/// Private on-disk registry under `browser-routines/<uuid>/routine.json`.
pub struct RoutineRegistry {
    root: PathBuf,
}

/// Exclusive per-routine lock. Dropped to release.
pub struct RoutineLock {
    _file: fs::File,
}

impl RoutineRegistry {
    #[must_use]
    pub(crate) fn directory(&self) -> &Path {
        &self.root
    }

    /// # Errors
    /// Returns [`RoutineError::Storage`] when the root cannot be created privately.
    pub fn open(root: PathBuf) -> Result<Self, RoutineError> {
        create_private_dir(&root)?;
        Ok(Self { root })
    }

    /// # Errors
    /// Validation, lock, or I/O failure. `routine_id` must match the directory name.
    pub fn save(&self, routine: &RoutineDefinition) -> Result<(), RoutineError> {
        routine.validate()?;
        let dir = self.root.join(routine.routine_id.to_string());
        create_private_dir(&dir)?;
        let _lock = self.lock(routine.routine_id)?;
        let path = dir.join("routine.json");
        let encoded =
            serde_json::to_vec_pretty(routine).map_err(|_| RoutineError::Json("malformed routine JSON".into()))?;
        write_private(&path, &encoded)
    }

    /// # Errors
    /// Missing file, malformed JSON, or validation failure.
    pub fn load(&self, routine_id: Uuid) -> Result<RoutineDefinition, RoutineError> {
        let path = self.root.join(routine_id.to_string()).join("routine.json");
        let bytes = read_private_file(&path)?;
        let routine: RoutineDefinition =
            serde_json::from_slice(&bytes).map_err(|_| RoutineError::Json("malformed routine JSON".into()))?;
        if routine.routine_id != routine_id {
            return Err(RoutineError::InvalidRecording);
        }
        routine.validate()?;
        Ok(routine)
    }

    /// # Errors
    /// I/O failure while reading the registry root.
    pub fn list(&self) -> Result<Vec<Uuid>, RoutineError> {
        let mut ids = Vec::new();
        let entries = fs::read_dir(&self.root).map_err(|_| RoutineError::Storage)?;
        for entry in entries {
            let entry = entry.map_err(|_| RoutineError::Storage)?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(id) = Uuid::parse_str(&name) else {
                continue;
            };
            if id.to_string() != name {
                continue;
            }
            let dir = entry.path();
            let Ok(dir_meta) = fs::symlink_metadata(&dir) else {
                continue;
            };
            if !dir_meta.is_dir() {
                continue;
            }
            let json = dir.join("routine.json");
            let Ok(json_meta) = fs::symlink_metadata(&json) else {
                continue;
            };
            if json_meta.is_file() {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Deletes the routine directory. Missing routines succeed.
    ///
    /// # Errors
    /// I/O failure while removing files.
    pub fn delete(&self, routine_id: Uuid) -> Result<(), RoutineError> {
        let dir = self.root.join(routine_id.to_string());
        match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(RoutineError::Storage),
            Ok(_) => {}
        }
        let lock = self.lock(routine_id)?;
        let staging = self.root.join(format!(".{routine_id}.deleting"));
        let rename = fs::rename(&dir, &staging);
        drop(lock);
        match rename {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(RoutineError::Storage),
            Ok(()) => {}
        }
        match fs::remove_dir_all(&staging) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RoutineError::Storage),
        }
    }

    /// # Errors
    /// I/O or lock failure.
    pub fn lock(&self, routine_id: Uuid) -> Result<RoutineLock, RoutineError> {
        let dir = self.root.join(routine_id.to_string());
        create_private_dir(&dir)?;
        let path = dir.join("lock");
        let mut options = fs::OpenOptions::new();
        options.create(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.share_mode(0).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options.open(&path).map_err(|_| RoutineError::Storage)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| RoutineError::Storage)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RoutineError::Storage);
        }
        #[cfg(unix)]
        {
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .map_err(|_| RoutineError::Storage)?;
        }
        file.set_len(0).map_err(|_| RoutineError::Storage)?;
        #[cfg(unix)]
        set_file_mode(&path, 0o600)?;
        Ok(RoutineLock { _file: file })
    }
}

pub(crate) fn create_private_dir(path: &Path) -> Result<(), RoutineError> {
    if let Some(parent) = path.parent() {
        validate_existing_ancestors(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(RoutineError::Storage),
        Ok(metadata) if !metadata.is_dir() => return Err(RoutineError::Storage),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(path)
                    .map_err(|_| RoutineError::Storage)?;
            }
            #[cfg(not(unix))]
            {
                fs::create_dir(path).map_err(|_| RoutineError::Storage)?;
            }
        }
        Err(_) => return Err(RoutineError::Storage),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| RoutineError::Storage)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RoutineError::Storage);
    }
    #[cfg(unix)]
    {
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(RoutineError::Storage);
        }
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).map_err(|_| RoutineError::Storage)?;
    }
    Ok(())
}

fn validate_existing_ancestors(path: &Path) -> Result<(), RoutineError> {
    let mut current = path.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if symlink_is_replaceable(&current, &metadata) {
                    return Err(RoutineError::Storage);
                }
                match fs::metadata(&current) {
                    Ok(target) => check_directory_permissions(&target)?,
                    Err(_) => return Err(RoutineError::Storage),
                }
            }
            Ok(metadata) => check_directory_permissions(&metadata)?,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(RoutineError::Storage),
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    Ok(())
}

fn check_directory_permissions(metadata: &fs::Metadata) -> Result<(), RoutineError> {
    if !metadata.is_dir() {
        return Err(RoutineError::Storage);
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode();
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err(RoutineError::Storage);
        }
    }
    Ok(())
}

/// True when the current user can replace this symlink (owned by us, parent
/// owned by us, or parent world-writable without sticky). System aliases such
/// as macOS `/tmp` → `/private/tmp` stay allowed because root owns both the
/// link and its parent.
fn symlink_is_replaceable(path: &Path, link: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        let uid = rustix::process::geteuid().as_raw();
        if link.uid() == uid {
            return true;
        }
        let Some(parent) = path.parent() else {
            return true;
        };
        match fs::metadata(parent) {
            Ok(metadata) if metadata.uid() == uid => true,
            Ok(metadata) => {
                let mode = metadata.permissions().mode();
                mode & 0o022 != 0 && mode & 0o1000 == 0
            }
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, link);
        true
    }
}

pub(crate) fn read_private_file(path: &Path) -> Result<Vec<u8>, RoutineError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Err(RoutineError::RoutineNotFound),
        Err(_) => return Err(RoutineError::Storage),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RoutineError::Storage);
    }
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(RoutineError::RoutineNotFound),
        Err(_) => Err(RoutineError::Storage),
    }
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), RoutineError> {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| file.write_all(bytes).and_then(|()| file.sync_all()), options)
        .map_err(|_| RoutineError::Storage)?;
    #[cfg(unix)]
    set_file_mode(path, 0o600)?;
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<(), RoutineError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| RoutineError::Storage)?;
    if metadata.file_type().is_symlink() {
        return Err(RoutineError::Storage);
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).map_err(|_| RoutineError::Storage)
}

#[cfg(test)]
mod tests {
    use super::RoutineRegistry;
    use crate::RoutineError;
    use crate::definition::tests::sample_definition;
    use uuid::Uuid;

    #[test]
    fn save_load_list_and_delete_round_trip() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let routine = sample_definition();
        registry.save(&routine).expect("save");
        let loaded = registry.load(routine.routine_id).expect("load");
        assert_eq!(loaded.name, "monthly-report");
        assert!(!loaded.is_ready());
        assert_eq!(registry.list().expect("list"), vec![routine.routine_id]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let file = temp
                .path()
                .join("routines")
                .join(routine.routine_id.to_string())
                .join("routine.json");
            let mode = std::fs::metadata(file).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        registry.delete(routine.routine_id).expect("delete");
        assert_eq!(registry.load(Uuid::nil()), Err(RoutineError::RoutineNotFound));
        assert!(registry.list().expect("list").is_empty());
    }

    #[cfg(unix)]
    fn privatize_temp(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(path).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("chmod");
    }

    #[cfg(not(unix))]
    fn privatize_temp(_path: &std::path::Path) {}

    #[test]
    fn lock_only_directories_are_not_listed() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let id = Uuid::from_u128(7);
        drop(registry.lock(id).expect("lock"));
        assert!(registry.list().expect("list").is_empty());
        assert_eq!(registry.load(id), Err(RoutineError::RoutineNotFound));
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_a_symlinked_ancestor() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let real = temp.path().join("real");
        std::fs::create_dir(&real).expect("real");
        privatize_temp(&real);
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert_eq!(
            RoutineRegistry::open(link.join("routines")).err(),
            Some(RoutineError::Storage)
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_rejects_a_user_owned_symlink_in_sticky_tmpdir() {
        let tmp = std::env::temp_dir();
        let suffix = Uuid::new_v4();
        let real = tmp.join(format!("horizon-routine-real-{suffix}"));
        let link = tmp.join(format!("horizon-routine-link-{suffix}"));
        std::fs::create_dir(&real).expect("real");
        privatize_temp(&real);
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let opened = RoutineRegistry::open(link.join("routines"));
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
        assert_eq!(opened.err(), Some(RoutineError::Storage));
    }

    #[cfg(unix)]
    #[test]
    fn lock_does_not_follow_or_truncate_a_symlink() {
        let temp = tempfile::tempdir().expect("temp");
        privatize_temp(temp.path());
        let registry = RoutineRegistry::open(temp.path().join("routines")).expect("open");
        let id = Uuid::from_u128(13);
        let dir = temp.path().join("routines").join(id.to_string());
        std::fs::create_dir(&dir).expect("dir");
        privatize_temp(&dir);
        let victim = temp.path().join("victim");
        std::fs::write(&victim, b"keep").expect("victim");
        std::os::unix::fs::symlink(&victim, dir.join("lock")).expect("symlink");
        assert_eq!(registry.lock(id).err(), Some(RoutineError::Storage));
        assert_eq!(std::fs::read(&victim).expect("read"), b"keep");
    }
}
