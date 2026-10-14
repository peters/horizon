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
pub(in crate::cloud_runtime) const ENROLLMENT_RECORD_VERSION: u32 = 4;
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
    /// Set once a relaunch reported that the profile no longer has this session's agent,
    /// so later reconnects stay quiet; cleared when the profile has the agent again.
    /// Omitted when unset, so other records keep their encoding.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed_agent_reported: bool,
}

impl Session {
    /// The session of a new panel: its tmux session has the panel's name, and it has no
    /// branch of its own.
    #[must_use]
    pub fn new(panel_id: &str, agent: &str, worktree: String) -> Self {
        Self {
            panel_id: panel_id.into(),
            agent: agent.into(),
            tmux: panel_id.into(),
            branch: String::new(),
            worktree,
            removed_agent_reported: false,
        }
    }
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
    /// Where the last successful deployment or reconnection spent its time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline: Option<super::timeline::Timeline>,
    /// The newest stop an agent asked for on this cloud's worker, as its checker last
    /// reported. Omitted when none, so earlier records keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_self_stop: Option<super::worker_contract::SelfStop>,
    /// Device identity observed after tailnet enrollment; reconnect refreshes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet_device: Option<super::tailnet::DeviceName>,
    /// Recorded device IDs and unresolved capture attempts survive worker replacement.
    #[serde(default, skip_serializing_if = "super::tailnet::receipt::EnrollmentLedger::is_empty")]
    pub tailnet_enrollment: super::tailnet::receipt::EnrollmentLedger,
    /// Same-worker siblings pinned before the image was built. Omitted when none, so
    /// single-repository records keep their encoding; a set without members is none.
    #[serde(
        default,
        deserialize_with = "super::siblings::Set::decode",
        skip_serializing_if = "Option::is_none"
    )]
    pub siblings: Option<super::siblings::Set>,
    /// Set when the worker's image was built from uncommitted changes, by New cloud or a
    /// rebuild. Omitted otherwise, so other records keep their encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted_image: Option<UncommittedImage>,
    /// Set when New cloud confirmed that the first image builds from the working copy,
    /// also when no file was uncommitted, so every attempt before that image is built
    /// lays exactly the confirmed files over the commit. Omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_copy: Option<super::repository::uncommitted::Confirmed>,
}

/// A rebuilt image whose build context held uncommitted files: the commit they change and
/// the hash of their content (`repository::uncommitted`), which the image's labels record too.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UncommittedImage {
    pub revision: String,
    pub hash: String,
}

impl Deployment {
    /// Whether the bound worker is ready for work over SSH: running, reachable, with its
    /// source in place and no stop requested.
    #[must_use]
    pub fn worker_ready(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            matches!(&self.operation, CreateState::Bound { worker_id } if worker_id == &worker.id)
                && self.stage == Stage::Ready
                && self.source_ready
                && !self.stop_requested
                && worker.desired_status == "RUNNING"
                && worker.ssh_endpoint().is_some()
        })
    }

    pub(in crate::cloud_runtime) fn record_version(&self) -> u32 {
        if !self.tailnet_enrollment.is_empty() {
            ENROLLMENT_RECORD_VERSION
        } else if self.siblings.is_some() {
            super::siblings::RECORD_VERSION
        } else {
            1
        }
    }

    /// Nonempty enrollment evidence fences readers that would ignore and erase it.
    pub(in crate::cloud_runtime) fn supported_version(&self) -> bool {
        self.version == self.record_version() && self.known_version_shape()
    }

    fn known_version_shape(&self) -> bool {
        match &self.siblings {
            None => {
                self.version == 1 || (self.version == ENROLLMENT_RECORD_VERSION && !self.tailnet_enrollment.is_empty())
            }
            Some(set) => {
                (self.version == super::siblings::RECORD_VERSION
                    || (self.version == ENROLLMENT_RECORD_VERSION && !self.tailnet_enrollment.is_empty()))
                    && set.fits()
            }
        }
    }

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
        Self::lock_existing(root)
    }
    /// As [`Self::lock`] for a cloud whose folder exists: a removed folder is not made
    /// again, and the lock fails with the folder's `NotFound` instead.
    /// # Errors
    /// As [`Self::lock`], and a missing folder.
    pub fn lock_existing(root: &Path) -> Result<Self> {
        crate::session_store::require_directory_durability()?;
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

    /// Whether this host keeps cloud records. Where it does not, no controller can lock
    /// or operate on one.
    #[must_use]
    pub fn supported() -> bool {
        crate::session_store::require_directory_durability().is_ok()
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// # Errors
    /// Returns read/parse errors without resetting or ignoring corrupt state.
    /// Refuses loading while a workspace-growth or compute-resize journal requires recovery.
    pub fn load(&self) -> Result<Option<Deployment>> {
        super::deployment::storage::growth::require_settled(self.root())?;
        super::deployment::storage::resize::require_settled(self.root())?;
        self.load_during_storage_growth()
    }

    pub(in crate::cloud_runtime) fn load_during_storage_growth(&self) -> Result<Option<Deployment>> {
        let path = self.root.join("deployment.json");
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut state: Deployment = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
                if !state.supported_version() {
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
        if !state.known_version_shape() {
            return Err(Error::Invalid(
                "Cloud state version does not match its enrollment evidence or siblings",
            ));
        }
        let mut record = state.clone();
        record.version = record.record_version();
        let bytes = serde_json::to_vec_pretty(&record).map_err(|_| Error::Json)?;
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
    #[test]
    // Cloud records need directory durability, which only Unix hosts have.
    #[cfg(unix)]
    fn a_lock_on_a_removed_folder_does_not_make_it_again() {
        let temp = tempfile::tempdir().unwrap();
        let removed = temp.path().join("removed");
        assert!(super::Store::lock_existing(&removed).is_err());
        assert!(!removed.exists(), "the folder is not made again");
        let kept = super::Store::lock(&removed).unwrap();
        drop(kept);
        assert!(
            super::Store::lock_existing(&removed).is_ok(),
            "an existing folder locks"
        );
    }

    use super::*;
    #[test]
    fn only_a_bound_running_reachable_worker_with_its_source_is_ready() {
        let ready = || -> Deployment {
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"ready-fixture","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
                "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"sessions":[],
                "source_ready":true,
                "worker":{"id":"worker1","name":"w","imageName":"i","desiredStatus":"RUNNING",
                    "publicIp":"203.0.113.7","portMappings":{"22":22022}}
            }))
            .unwrap()
        };
        assert!(ready().worker_ready());
        let changes: [fn(&mut Deployment); 6] = [
            |state| state.stage = Stage::Stopping,
            |state| state.source_ready = false,
            |state| state.stop_requested = true,
            |state| {
                state.operation = CreateState::Bound {
                    worker_id: "worker2".into(),
                }
            },
            |state| state.worker.as_mut().unwrap().desired_status = "EXITED".into(),
            |state| state.worker.as_mut().unwrap().port_mappings = None,
        ];
        for change in changes {
            let mut state = ready();
            change(&mut state);
            assert!(!state.worker_ready(), "{state:?}");
        }
    }
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
        assert!(state.siblings.is_none());
        assert!(state.tailnet_device.is_none());
    }

    #[test]
    #[cfg(unix)] // Store::lock requires Unix directory durability.
    fn tailnet_identity_survives_legacy_migration_and_refresh() {
        use super::super::allocation::{AllocationId, ControllerId, ProjectId, ProjectIdentity, legacy::Records};

        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
        state.cloud_id = "legacy-cloud".into();
        let actual = super::super::tailnet::DeviceName {
            name: "renamed-worker-1.example.ts.net".into(),
            observed: true,
        };
        state.tailnet_device = Some(actual.clone());
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap().unwrap().tailnet_device, Some(actual.clone()));
        let identity = ProjectIdentity::new(
            ProjectId::generate(),
            "saved-session".into(),
            "workspace".into(),
            state.cloud_id.clone(),
        )
        .unwrap();
        let records = Records::from_legacy(
            &serde_json::to_vec(&state).unwrap(),
            identity,
            AllocationId::generate(),
            ControllerId::generate(),
        )
        .unwrap();
        let restored = Records::decode(&records.allocation_bytes().unwrap(), &records.project_bytes().unwrap())
            .unwrap()
            .deployment();
        assert_eq!(restored.tailnet_device, Some(actual));
        state.tailnet_device = None;
        store.save(&state).unwrap();
        assert!(store.load().unwrap().unwrap().tailnet_device.is_none());
    }

    #[test]
    #[cfg(unix)] // Store::lock needs durable directory updates, which only Unix hosts have.
    fn only_the_sibling_record_version_loads_pinned_siblings() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
        let mut set = super::super::siblings::Set {
            primary_directory: "app".into(),
            members: vec![super::super::siblings::Sibling {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                revision: "b".repeat(40),
                image_revision: None,
                local_repository: "/synthetic/native-lib".into(),
                profile: "gpu".into(),
            }],
        };
        let write = |state: &Deployment| {
            assert!(store.save(state).is_err(), "an unloadable record is never saved");
            std::fs::write(root.path().join("deployment.json"), serde_json::to_vec(state).unwrap()).unwrap();
        };
        state.siblings = Some(set.clone());
        write(&state);
        assert!(store.load().is_err(), "version 1 never carries siblings");
        state.version = super::super::siblings::RECORD_VERSION;
        store.save(&state).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.siblings, state.siblings);
        let encoded = serde_json::to_value(&loaded).unwrap();
        assert_eq!(encoded["siblings"]["members"][0]["directory"], "native-lib");
        state.siblings = None;
        write(&state);
        assert!(store.load().is_err(), "the sibling version needs siblings");
        set.members.clear();
        state.siblings = Some(set);
        assert!(
            store.save(&state).is_err(),
            "a set without members would not load again"
        );
        state.version = 1;
        write(&state);
        assert_eq!(
            store.load().unwrap().unwrap().siblings,
            None,
            "a set without members is no siblings"
        );
    }

    #[test]
    #[cfg(unix)]
    fn enrollment_evidence_automatically_fences_old_readers_with_or_without_siblings() {
        use super::super::tailnet::receipt::{EnrollmentLedger, tests::populated};
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let sibling = super::super::siblings::Set {
            primary_directory: "app".into(),
            members: vec![super::super::siblings::Sibling {
                alias: "native".into(),
                repository: "example/native-lib".into(),
                directory: "native-lib".into(),
                revision: "b".repeat(40),
                image_revision: None,
                local_repository: "/synthetic/native-lib".into(),
                profile: "gpu".into(),
            }],
        };
        let mut pending = EnrollmentLedger::default();
        pending.arm(populated().attempts()[0].clone()).unwrap();
        for siblings in [None, Some(sibling)] {
            for ledger in [populated(), pending.clone()] {
                let mut state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
                state.siblings = siblings.clone();
                state.version = state.record_version();
                let previous_version = state.version;
                state.tailnet_enrollment = ledger.clone();
                store.save(&state).unwrap();
                let loaded = store.load().unwrap().unwrap();
                assert_eq!(loaded.version, ENROLLMENT_RECORD_VERSION);
                assert_eq!(loaded.tailnet_enrollment, ledger);
                assert_eq!(loaded.siblings, siblings);
                // The exact pre-ledger acceptance rule rejects the new saved encoding.
                let previous_reader_accepts = match loaded.siblings {
                    None => loaded.version == 1,
                    Some(ref set) => loaded.version == super::super::siblings::RECORD_VERSION && set.fits(),
                };
                assert!(!previous_reader_accepts);
                state.version = previous_version;
                std::fs::write(root.path().join("deployment.json"), serde_json::to_vec(&state).unwrap()).unwrap();
                assert!(store.load().is_err(), "old versions cannot contain enrollment evidence");
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn empty_or_unknown_enrollment_versions_are_refused_without_resetting_state() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap().unwrap().version, 1);
        for version in [0, 2, ENROLLMENT_RECORD_VERSION, 99] {
            state.version = version;
            let bytes = serde_json::to_vec(&state).unwrap();
            std::fs::write(root.path().join("deployment.json"), &bytes).unwrap();
            assert!(store.load().is_err());
            assert!(store.save(&state).is_err());
            assert_eq!(std::fs::read(root.path().join("deployment.json")).unwrap(), bytes);
        }
    }

    #[test]
    #[cfg(unix)]
    fn enrollment_evidence_survives_actual_paired_migration_and_reopen() {
        use super::super::{allocation::ControllerId, tailnet::receipt::tests::populated};
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("golden");
        let mut state: Deployment = serde_json::from_str(LEGACY_ENCODING).unwrap();
        state.tailnet_enrollment = populated();
        Store::lock(&root).unwrap().save(&state).unwrap();
        let controller = ControllerId::generate();
        {
            let mut migrated =
                migration::MigratedStore::migrate(&root, "saved-session", "workspace", controller).unwrap();
            let records = migrated.load().unwrap();
            assert_eq!(records.deployment().tailnet_enrollment, state.tailnet_enrollment);
            assert_eq!(records.deployment().version, ENROLLMENT_RECORD_VERSION);
            migrated.save(&records).unwrap();
        }
        let mut reopened = migration::MigratedStore::migrate(&root, "saved-session", "workspace", controller).unwrap();
        assert_eq!(
            reopened.load().unwrap().deployment().tailnet_enrollment,
            state.tailnet_enrollment
        );
    }
}
