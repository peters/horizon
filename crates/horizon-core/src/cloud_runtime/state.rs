//! Durable allocation fence and per-cloud ownership lock.
pub mod migration;
mod replacement;
mod transaction;
use super::{Error, Result, Stage};
use horizon_cloud::{CreateState, Worker, WorkerSpec};
pub use horizon_cloud_protocol::OperationId;
#[cfg(all(test, unix))]
pub(super) use replacement::REPLACEMENT_PENDING;
pub use replacement::{ImageReplacement, ReplacementImage, ReplacementPhase};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub panel_id: String,
    pub agent: String,
    pub tmux: String,
    pub branch: String,
    pub worktree: String,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ReadyHistory {
    #[default]
    Unobserved,
    Observed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Deployment {
    pub version: u32,
    pub cloud_id: String,
    pub repository: PathBuf,
    pub revision: String,
    pub profile: horizon_cloud::Profile,
    pub stage: Stage,
    pub operation: CreateState,
    pub spec: Option<WorkerSpec>,
    /// Retains private-image intent across failed preparation and settings changes.
    #[serde(default)]
    pub registry_generation: Option<String>,
    pub worker: Option<Worker>,
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub source_ready: bool,
    /// Successful attempt through worker/session readiness, excluding earlier failed attempts.
    /// Not a measurement of application startup or the first visible frame.
    #[serde(default)]
    pub ready_after_seconds: Option<u64>,
    /// Retains unknown legacy readiness history through a failed reconnect.
    #[serde(default)]
    pub ready_history: ReadyHistory,
    #[serde(default)]
    pub stop_requested: bool,
    #[serde(default)]
    pub browserstack_released: bool,
    #[serde(default)]
    pub browserstack_targets: std::collections::BTreeSet<String>,
    /// Journals a switch of the bound worker to a rebuilt image until it commits or is cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_replacement: Option<ImageReplacement>,
    /// A committed image replacement reset the worker's container; its sessions must relaunch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_restart: Option<OperationId>,
}

impl Deployment {
    pub(super) fn normalize_readiness_history(&mut self) {
        if self.stage == Stage::Ready {
            self.ready_history = ReadyHistory::Observed;
        }
    }

    #[must_use]
    pub fn requires_browserstack_release(&self) -> bool {
        self.profile.capabilities.browserstack.is_some() && !self.browserstack_released
    }
    /// CPU and memory can change until a worker is requested.
    /// `RunPod` cannot resize an existing pod.
    #[must_use]
    pub fn resizable(&self) -> bool {
        self.stage != Stage::Deleted && self.operation == CreateState::Prepared && self.worker.is_none()
    }

    /// A deleted cloud can choose the next worker's size, but that choice stays
    /// on the launch profile until redeploy releases the old storage journal.
    #[must_use]
    pub fn accepts_next_size(&self) -> bool {
        self.resizable()
            || (self.stage == Stage::Deleted
                && self.worker.is_none()
                && matches!(self.operation, CreateState::Prepared | CreateState::Terminated { .. }))
    }
}

pub struct Store {
    root: PathBuf,
    lock_file: File,
}
/// # Errors
/// Rejects persisted identities that are not a single portable path component.
pub fn cloud_directory(root: &Path, cloud_id: &str) -> Result<PathBuf> {
    if !horizon_cloud::valid_id(cloud_id) {
        return Err(Error::Invalid("Invalid cloud identity"));
    }
    Ok(root.join(cloud_id))
}
impl Store {
    /// # Errors
    /// Refuses simultaneous controllers and platforms without supported directory durability.
    /// OS locks release after a crash. Unsupported hosts fail before state or provider mutation.
    pub fn lock(root: &Path) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
        std::fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("operation.lock"))?;
        file.try_lock().map_err(|_| Error::Busy)?;
        Ok(Self {
            root: root.into(),
            lock_file: file,
        })
    }
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// # Errors
    /// Returns read/parse errors without resetting or ignoring corrupt state.
    pub fn load(&self) -> Result<Option<Deployment>> {
        let path = self.root.join("deployment.json");
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut state: Deployment = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
                if state.version != 1 {
                    return Err(Error::Invalid("Unsupported cloud state"));
                }
                state.normalize_readiness_history();
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    /// # Errors
    /// Persist cleanup intent before a credential transfer can become uncertain.
    pub fn arm_browserstack(&self, state: &mut Deployment) -> Result<()> {
        state.browserstack_released = false;
        self.save(state)
    }
    /// # Errors
    /// Syncs file contents and parent directory before returning to the provider.
    pub fn save(&self, state: &Deployment) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|_| Error::Json)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(self.root.join("deployment.json")).map_err(|e| e.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(not(unix))]
    fn unsupported_cloud_control_does_not_create_or_modify_state() {
        let root = tempfile::tempdir().unwrap();
        let absent = root.path().join("new-state");
        assert!(Store::lock(&absent).is_err());
        assert!(!absent.exists());
        let existing = root.path().join("deployment.json");
        std::fs::write(&existing, b"preserve existing record").unwrap();
        assert!(Store::lock(root.path()).is_err());
        assert_eq!(std::fs::read(existing).unwrap(), b"preserve existing record");
        assert!(!root.path().join("operation.lock").exists());
    }

    #[test]
    fn persisted_cloud_ids_cannot_escape_the_state_root() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["", "..", "../outside", "/outside", "C:\\outside", "a/b", "a\\b"] {
            assert!(cloud_directory(temp.path(), id).is_err(), "accepted {id:?}");
        }
        assert_eq!(
            cloud_directory(temp.path(), "cloud-123").unwrap(),
            temp.path().join("cloud-123")
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    #[cfg(unix)]
    fn competing_controllers_cannot_both_hold_operation_lock() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::lock(temp.path()).unwrap();
        assert!(matches!(Store::lock(temp.path()), Err(Error::Busy)));
        drop(store);
        assert!(Store::lock(temp.path()).is_ok());
    }

    /// `deployment.json` as written before image replacement existed.
    const LEGACY_ENCODING: &str = r#"{
  "version": 1,
  "cloud_id": "golden",
  "repository": "/synthetic",
  "revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "profile": {
    "provider": "runpod",
    "image": "registry.example/worker",
    "cpu": 4,
    "memory_gb": 8,
    "gpu": false,
    "build": null,
    "storage": {
      "container_gb": 20,
      "volume_gb": 20
    },
    "bootstrap": {
      "readiness_seconds": 600,
      "contract_version": 1
    },
    "capabilities": {
      "agents": [
        "codex",
        "claude",
        "grok"
      ],
      "browsers": [
        "chromium"
      ],
      "desktop": true
    }
  },
  "stage": "Ready",
  "operation": {
    "state": "bound",
    "worker_id": "worker1"
  },
  "spec": null,
  "registry_generation": null,
  "worker": null,
  "sessions": [],
  "source_ready": false,
  "ready_after_seconds": null,
  "ready_history": "Unobserved",
  "stop_requested": false,
  "browserstack_released": false,
  "browserstack_targets": []
}"#;

    #[test]
    fn records_without_a_replacement_keep_their_exact_encoding() {
        let state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
        assert!(state.image_replacement.is_none() && state.session_restart.is_none());
        let encoded = serde_json::to_vec_pretty(&state).unwrap();
        assert_eq!(String::from_utf8(encoded).unwrap(), LEGACY_ENCODING);
    }
}
