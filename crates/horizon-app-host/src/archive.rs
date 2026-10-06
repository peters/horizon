//! Explicitly retained private run evidence, independent of rolling interactive captures.
use crate::{Error, Result, runner};
use horizon_app_process::storage::Directory;
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

pub struct Archive {
    path: PathBuf,
    directory: Directory,
    usage: Mutex<(usize, usize)>,
}
pub struct Store {
    path: PathBuf,
    directory: Directory,
    admission: Mutex<()>,
}
impl Store {
    /// # Errors
    /// Retain the reports root capability before any asynchronous work begins.
    pub fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            path: root.to_owned(),
            directory: Directory::open(root)?,
            admission: Mutex::new(()),
        })
    }
    /// # Errors
    /// A replaced root cannot redirect a later run.
    pub fn create(&self) -> Result<Archive> {
        let _admission = self.admission.lock().map_err(|_| Error::Unavailable)?;
        self.directory.matches_path(&self.path)?;
        if self.directory.count_children(8)? >= 8 {
            return Err(Error::EvidenceFull);
        }
        let name = Uuid::new_v4().simple().to_string();
        self.directory.create_child(&name)?;
        let directory = self.directory.child(&name)?;
        self.directory.matches_path(&self.path)?;
        Ok(Archive {
            path: self.path.join(name),
            directory,
            usage: Mutex::new((0, 0)),
        })
    }
}
impl Archive {
    /// # Errors
    /// Export validated controller evidence through the same retained budget.
    pub fn capture(&self, session: Uuid, kind: runner::CaptureKind, bytes: &[u8]) -> Result<runner::Evidence> {
        match kind {
            runner::CaptureKind::Screenshot => self.screenshot(session, bytes),
            runner::CaptureKind::Provider(kind) => self.media(kind, bytes),
        }
    }
    /// # Errors
    /// Provider adapter validates and redacts evidence before this private export.
    pub fn media(&self, kind: horizon_app_provider::media::Kind, bytes: &[u8]) -> Result<runner::Evidence> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::Unavailable);
        }
        let id = Uuid::new_v4();
        let path = self.write(&format!("{}.{}", id.simple(), kind.extension()), bytes)?;
        Ok(runner::Evidence {
            id,
            path: Some(path),
            bytes: bytes.len(),
            state: runner::EvidenceState::Available,
        })
    }
    /// # Errors
    /// Create one private run directory beneath an existing anchored owned root.
    pub fn new(root: &Path) -> Result<Self> {
        Store::new(root)?.create()
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let mut usage = self.usage.lock().map_err(|_| Error::Unavailable)?;
        let total = usage.1.checked_add(bytes.len()).ok_or(Error::Unavailable)?;
        let maximum = if name == "report.json" { 128 } else { 120 };
        if usage.0 >= 1025 || total > maximum * 1024 * 1024 {
            return Err(Error::Unavailable);
        }
        self.directory.matches_path(&self.path)?;
        let mut file = self.directory.new_file(name)?;
        // Failed writes still consume their reserved budget and remain explicit private partial evidence.
        usage.0 += 1;
        usage.1 = total;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Unavailable)?;
        self.directory.matches_path(&self.path)?;
        Ok(self.path.join(name))
    }
    /// # Errors
    /// Native capture validation belongs to the controller; archives accept its bounded PNG only.
    pub fn screenshot(&self, _session: Uuid, png: &[u8]) -> Result<runner::Evidence> {
        if png.is_empty() || png.len() > 8 * 1024 * 1024 {
            return Err(Error::Unavailable);
        }
        let id = Uuid::new_v4();
        let path = self.write(&format!("{}.png", id.simple()), png)?;
        Ok(runner::Evidence {
            id,
            path: Some(path),
            bytes: png.len(),
            state: runner::EvidenceState::Available,
        })
    }
    /// # Errors
    /// Save the terminal report without replacing prior evidence. Evidence is retained after host exit.
    pub fn finish(&self, report: &runner::Report) -> Result<Value> {
        self.finish_value(serde_json::to_value(report).map_err(|_| Error::Unavailable)?)
    }
    /// # Errors
    /// The retained worker records a terminal outcome even if its CLI/MCP listener has disappeared.
    pub fn finish_result(&self, result: Result<runner::Report>) -> Result<Value> {
        match result {
            Ok(report) => self.finish(&report),
            Err(error) => {
                self.finish_value(serde_json::json!({"run_error":error.to_string()}))?;
                Err(error)
            }
        }
    }
    fn finish_value(&self, mut value: Value) -> Result<Value> {
        value.as_object_mut().ok_or(Error::Unavailable)?.insert(
            "report_path".into(),
            serde_json::to_value(self.path.join("report.json")).map_err(|_| Error::Unavailable)?,
        );
        let bytes = serde_json::to_vec_pretty(&value).map_err(|_| Error::Unavailable)?;
        self.write("report.json", &bytes)?;
        Ok(value)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn persisted_archive_admission_counts_partial_runs_and_saves_terminal_errors() {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Store::new(root.path()).unwrap();
        for _ in 0..8 {
            let archive = store.create().unwrap();
            assert!(archive.finish_result(Err(Error::Cancelled)).is_err());
            assert!(archive.path.join("report.json").is_file());
        }
        drop(store);
        assert!(matches!(
            Store::new(root.path()).unwrap().create(),
            Err(Error::EvidenceFull)
        ));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 8);
    }
    #[test]
    fn archived_capture_survives_shutdown_and_cannot_follow_root_replacement() {
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let archive = Archive::new(root.path()).unwrap();
        let capture = archive.screenshot(Uuid::new_v4(), b"validated-fixture").unwrap();
        let path = capture.path.unwrap();
        drop(archive);
        assert_eq!(std::fs::read(&path).unwrap(), b"validated-fixture");
        let archive = Archive::new(root.path()).unwrap();
        let original = archive.path.with_extension("saved");
        std::fs::rename(&archive.path, &original).unwrap();
        std::fs::create_dir(&archive.path).unwrap();
        std::fs::set_permissions(&archive.path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(archive.screenshot(Uuid::new_v4(), b"validated-fixture").is_err());
        assert_eq!(std::fs::read_dir(&archive.path).unwrap().count(), 0);
    }
}
