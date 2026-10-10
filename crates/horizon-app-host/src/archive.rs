//! Explicitly retained private run evidence, independent of rolling interactive captures.
use crate::{Error, HostFailure, Result, runner};
use horizon_app_process::storage::Directory;
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

pub(crate) const MAX_EVIDENCE_FILES: usize = 1024;
// Two 145-step iPhone lanes can retain over 600 MiB before provider media.
const EVIDENCE_BYTES: usize = 1024 * 1024 * 1024;
const REPORT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Usage {
    files: usize,
    bytes: usize,
    report_reserved: bool,
}

pub struct Archive {
    path: PathBuf,
    directory: Directory,
    usage: Mutex<Usage>,
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
        let _admission = self
            .admission
            .lock()
            .map_err(|_| Error::host_lock(crate::lifecycle::Lock::ArchiveAdmission))?;
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
            usage: Mutex::new(Usage::default()),
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
            return Err(HostFailure::CaptureInvalid.into());
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
        let mut usage = self.usage.lock().map_err(|_| HostFailure::ArchiveState)?;
        let report = name == "report.json";
        let total = usage.bytes.checked_add(bytes.len()).ok_or(HostFailure::EvidenceBytes)?;
        if report {
            if usage.report_reserved || bytes.len() > REPORT_BYTES {
                return Err(HostFailure::ReportLimit.into());
            }
        } else if usage.files >= MAX_EVIDENCE_FILES {
            return Err(HostFailure::EvidenceFiles.into());
        } else if total > EVIDENCE_BYTES {
            return Err(HostFailure::EvidenceBytes.into());
        }
        self.directory.matches_path(&self.path)?;
        let mut file = self.directory.new_file(name)?;
        // Failed writes consume reservations; evidence never consumes the terminal report reserve.
        if report {
            usage.report_reserved = true;
        } else {
            usage.files += 1;
            usage.bytes = total;
        }
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| HostFailure::ArchiveWrite(error.kind()))?;
        self.directory.matches_path(&self.path)?;
        Ok(self.path.join(name))
    }
    /// # Errors
    /// Native capture validation belongs to the controller; archives accept its bounded PNG only.
    pub fn screenshot(&self, _session: Uuid, png: &[u8]) -> Result<runner::Evidence> {
        if png.is_empty() || png.len() > 8 * 1024 * 1024 {
            return Err(HostFailure::CaptureInvalid.into());
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
        self.finish_value(serde_json::to_value(report).map_err(|_| HostFailure::ArchiveSerialization)?)
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
        value.as_object_mut().ok_or(HostFailure::ArchiveSerialization)?.insert(
            "report_path".into(),
            serde_json::to_value(self.path.join("report.json")).map_err(|_| HostFailure::ArchiveSerialization)?,
        );
        let bytes = serde_json::to_vec_pretty(&value).map_err(|_| HostFailure::ArchiveSerialization)?;
        self.write("report.json", &bytes)?;
        Ok(value)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn exhausted_evidence_preserves_one_bounded_terminal_report() {
        let temp_root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp_root.path().canonicalize().unwrap();
        let archive = Archive::new(&root).unwrap();
        for _ in 0..MAX_EVIDENCE_FILES {
            archive.screenshot(Uuid::new_v4(), b"validated-fixture").unwrap();
        }
        assert_eq!(
            archive.screenshot(Uuid::new_v4(), b"one-too-many").err(),
            Some(HostFailure::EvidenceFiles.into())
        );
        assert!(archive.finish_result(Err(Error::Cancelled)).is_err());
        let value: Value = serde_json::from_slice(&std::fs::read(archive.path.join("report.json")).unwrap()).unwrap();
        assert!(value["run_error"].as_str().unwrap().contains("app_run_cancelled"));
        assert_eq!(
            std::fs::read_dir(&archive.path).unwrap().count(),
            MAX_EVIDENCE_FILES + 1
        );
        assert!(archive.finish_value(serde_json::json!({})).is_err());

        let archive = Archive::new(&root).unwrap();
        archive.usage.lock().unwrap().bytes = EVIDENCE_BYTES;
        assert_eq!(
            archive.screenshot(Uuid::new_v4(), b"over-byte-budget").err(),
            Some(HostFailure::EvidenceBytes.into())
        );
        assert!(
            archive
                .finish_value(serde_json::json!({"large": "x".repeat(REPORT_BYTES)}))
                .is_err()
        );
        assert!(
            archive
                .finish_value(serde_json::json!({"cleanup_confirmed": true}))
                .is_ok()
        );
    }

    #[test]
    fn two_long_iphone_lanes_fit_the_evidence_budget_without_removing_the_limit() {
        let temp_root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let archive = Archive::new(&temp_root.path().canonicalize().unwrap()).unwrap();
        // The original two-lane run averaged about 2 MiB for each PNG. Model the
        // retained reservations, then exercise real admission on either side of the cap.
        let screenshots = 145 * 2;
        let bytes = screenshots * 2 * 1024 * 1024;
        assert!(bytes > 120 * 1024 * 1024);
        {
            let mut usage = archive.usage.lock().unwrap();
            usage.files = screenshots;
            usage.bytes = bytes;
        }
        assert!(archive.screenshot(Uuid::new_v4(), b"next-validated-png").is_ok());
        archive.usage.lock().unwrap().bytes = EVIDENCE_BYTES;
        assert_eq!(
            archive.screenshot(Uuid::new_v4(), b"over-limit").err(),
            Some(HostFailure::EvidenceBytes.into())
        );
        assert!(
            archive
                .finish_value(serde_json::json!({"cleanup_confirmed":true}))
                .is_ok()
        );
    }

    #[test]
    fn persisted_archive_admission_counts_partial_runs_and_saves_terminal_errors() {
        let temp_root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp_root.path().canonicalize().unwrap();
        let store = Store::new(&root).unwrap();
        for _ in 0..8 {
            let archive = store.create().unwrap();
            assert!(archive.finish_result(Err(Error::Cancelled)).is_err());
            assert!(archive.path.join("report.json").is_file());
        }
        drop(store);
        assert!(matches!(Store::new(&root).unwrap().create(), Err(Error::EvidenceFull)));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 8);
    }
    #[test]
    fn archived_capture_survives_shutdown_and_cannot_follow_root_replacement() {
        let temp_root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp_root.path().canonicalize().unwrap();
        let archive = Archive::new(&root).unwrap();
        let capture = archive.screenshot(Uuid::new_v4(), b"validated-fixture").unwrap();
        let path = capture.path.unwrap();
        drop(archive);
        assert_eq!(std::fs::read(&path).unwrap(), b"validated-fixture");
        let archive = Archive::new(&root).unwrap();
        let original = archive.path.with_extension("saved");
        std::fs::rename(&archive.path, &original).unwrap();
        std::fs::create_dir(&archive.path).unwrap();
        std::fs::set_permissions(&archive.path, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(archive.screenshot(Uuid::new_v4(), b"validated-fixture").is_err());
        assert_eq!(std::fs::read_dir(&archive.path).unwrap().count(), 0);
    }

    #[test]
    fn poisoned_archive_admission_retains_its_typed_lifecycle_cause() {
        let temp_root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Store::new(&temp_root.path().canonicalize().unwrap()).unwrap();
        assert!(
            std::panic::catch_unwind(|| {
                let _guard = store.admission.lock().unwrap();
                panic!("synthetic admission poison");
            })
            .is_err()
        );
        assert!(matches!(
            store.create(),
            Err(Error::LifecycleUnavailable(crate::lifecycle::Fault::LockPoisoned {
                resource: crate::lifecycle::Lock::ArchiveAdmission
            }))
        ));
        assert_eq!(std::fs::read_dir(temp_root.path()).unwrap().count(), 0);
    }
}
