use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use atomicwrites::{AllowOverwrite, AtomicFile};
use uuid::Uuid;

use crate::RoutineError;
use crate::definition::RoutineDefinition;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

/// Private on-disk registry under `browser-routines/<uuid>/routine.json`.
pub struct RoutineRegistry {
    root: PathBuf,
}

/// Exclusive per-routine lock. Dropped to release.
pub struct RoutineLock {
    _file: fs::File,
}

impl RoutineRegistry {
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
        let bytes = fs::read(&path).map_err(|_| RoutineError::RoutineNotFound)?;
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
            if let Ok(id) = Uuid::parse_str(&entry.file_name().to_string_lossy()) {
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
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RoutineError::Storage),
        }
    }

    /// # Errors
    /// I/O or lock failure.
    pub fn lock(&self, routine_id: Uuid) -> Result<RoutineLock, RoutineError> {
        let dir = self.root.join(routine_id.to_string());
        create_private_dir(&dir)?;
        let path = dir.join("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|_| RoutineError::Storage)?;
        #[cfg(unix)]
        {
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .map_err(|_| RoutineError::Storage)?;
            let metadata = fs::metadata(&path).map_err(|_| RoutineError::Storage)?;
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            fs::set_permissions(&path, permissions).map_err(|_| RoutineError::Storage)?;
        }
        Ok(RoutineLock { _file: file })
    }
}

fn create_private_dir(path: &Path) -> Result<(), RoutineError> {
    #[cfg(unix)]
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|_| RoutineError::Storage)?;
        let metadata = fs::metadata(path).map_err(|_| RoutineError::Storage)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).map_err(|_| RoutineError::Storage)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path).map_err(|_| RoutineError::Storage)
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), RoutineError> {
    let file = AtomicFile::new(path, AllowOverwrite);
    file.write(|handle| handle.write_all(bytes))
        .map_err(|_| RoutineError::Storage)?;
    #[cfg(unix)]
    {
        let metadata = fs::metadata(path).map_err(|_| RoutineError::Storage)?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions).map_err(|_| RoutineError::Storage)?;
    }
    Ok(())
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
}
