//! Private bounded screenshot retention. Tool inputs never supply file paths.
use crate::{Error, Result};
use horizon_app_process::storage::Directory;
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;
#[derive(Serialize)]
pub struct Capture {
    pub id: Uuid,
    pub session: Uuid,
    pub path: PathBuf,
    pub bytes: usize,
}
pub struct Evidence {
    root: PathBuf,
    directory: Directory,
    retained: Mutex<VecDeque<String>>,
}
impl Evidence {
    pub fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            root: root.to_owned(),
            directory: Directory::open(root)?,
            retained: Mutex::new(VecDeque::new()),
        })
    }
    pub fn screenshot(&self, session: Uuid, png: &[u8]) -> Result<Capture> {
        self.screenshot_with_creator(session, png, |directory, name, retained| {
            directory
                .create_child_tracked(name, || retained.push_back(name.to_owned()))
                .map_err(Error::from)
        })
    }
    fn screenshot_with_creator(
        &self,
        session: Uuid,
        png: &[u8],
        create: impl FnOnce(&Directory, &str, &mut VecDeque<String>) -> Result<()>,
    ) -> Result<Capture> {
        if png.is_empty() || png.len() > 8 * 1024 * 1024 {
            return Err(Error::Unavailable);
        }
        let mut retained = self.retained.lock().map_err(|_| Error::Unavailable)?;
        self.directory.matches_path(&self.root)?;
        while retained.len() >= 32 {
            let name = retained.front().ok_or(Error::Unavailable)?;
            self.directory.retire_child(name)?;
            retained.pop_front();
        }
        let id = Uuid::new_v4();
        let name = id.simple().to_string();
        // Register every new directory before parent synchronization, including failed incomplete exports.
        create(&self.directory, &name, &mut retained)?;
        let path = self.root.join(&name);
        let directory = self.directory.child(&name)?;
        let mut file = directory.new_file("frame.png")?;
        file.write_all(png)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Unavailable)?;
        self.directory.matches_path(&self.root)?;
        directory.matches_path(&path)?;
        Ok(Capture {
            id,
            session,
            path: path.join("frame.png"),
            bytes: png.len(),
        })
    }
}
impl Drop for Evidence {
    fn drop(&mut self) {
        if let Ok(retained) = self.retained.get_mut() {
            for name in retained {
                let _ = self.directory.retire_child(name);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    #[test]
    fn bounded_exports_retire_only_owned_files_and_close_removes_remaining_captures() {
        let root = root();
        let path = root.path().canonicalize().unwrap();
        std::fs::write(path.join("retained-host-file"), b"host-owned").unwrap();
        let evidence = Evidence::new(&path).unwrap();
        let session = Uuid::new_v4();
        let first = evidence.screenshot(session, b"synthetic-png").unwrap();
        for _ in 0..32 {
            evidence.screenshot(session, b"synthetic-png").unwrap();
        }
        assert!(!first.path.exists());
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 33);
        drop(evidence);
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
        assert_eq!(std::fs::read(path.join("retained-host-file")).unwrap(), b"host-owned");
    }
    #[test]
    fn replacing_the_export_root_cannot_redirect_a_capture_or_cleanup() {
        let parent = root();
        let path = parent.path().canonicalize().unwrap().join("evidence");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let evidence = Evidence::new(&path).unwrap();
        let first = evidence.screenshot(Uuid::new_v4(), b"synthetic-png").unwrap();
        let saved = path.with_file_name("original-evidence");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let foreign = path.join(first.id.simple().to_string());
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("foreign-file"), b"foreign").unwrap();
        assert!(evidence.screenshot(Uuid::new_v4(), b"synthetic-png").is_err());
        drop(evidence);
        assert_eq!(std::fs::read_dir(&saved).unwrap().count(), 0);
        assert_eq!(std::fs::read(foreign.join("foreign-file")).unwrap(), b"foreign");
    }

    #[test]
    fn failure_after_mkdir_keeps_incomplete_exports_owned_and_bounded() {
        let root = root();
        let path = root.path().canonicalize().unwrap();
        let evidence = Evidence::new(&path).unwrap();
        for _ in 0..40 {
            assert!(
                evidence
                    .screenshot_with_creator(Uuid::new_v4(), b"synthetic-png", |directory, name, retained| {
                        directory.create_child_tracked(name, || retained.push_back(name.to_owned()))?;
                        Err(Error::Unavailable)
                    })
                    .is_err()
            );
        }
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 32);
        assert_eq!(evidence.retained.lock().unwrap().len(), 32);
        drop(evidence);
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    }
}
