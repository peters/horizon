use super::*;
use crate::{HorizonHome, remote_workspace::RemoteWorkspaceState};
#[cfg(target_os = "linux")]
use crate::{
    cloud_run::{ArtifactDigest, CloudProvider, interactive_worker::*},
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase},
};
#[cfg(target_os = "linux")]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cfg(target_os = "linux")]
mod admission;
#[cfg(target_os = "linux")]
mod lifecycle;
#[cfg(target_os = "linux")]
mod network_volume;
#[cfg(target_os = "linux")]
mod races;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    dormant: StoredRemoteWorkspace,
    profile: RunPodProfile,
    key: RunPodApiKey,
    #[cfg(target_os = "linux")]
    remote: Arc<Remote>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private fixture");
        }
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1, "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": {"provider": "run_pod", "profile": "development", "disk_gib": 20,
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)), "lifetime": "persistent"},
                "repository": {"repository": "example/project", "commit": "b".repeat(40)}
            }
        }))
        .expect("workspace");
        let dormant = store.create_remote_workspace(OWNER, &state).expect("dormant");
        Self {
            directory,
            identities: RemoteSshIdentityStore::new(&home),
            store,
            dormant,
            profile: serde_json::from_value(serde_json::json!({
                "name": "development", "gpu_type_ids": ["synthetic-gpu"], "gpu_count": 1,
                "ports": ["22/tcp"], "volume_gib": 0
            }))
            .expect("profile"),
            key: RunPodApiKey::new("synthetic-private-credential").expect("key"),
            #[cfg(target_os = "linux")]
            remote: Arc::default(),
        }
    }

    fn counts(&self) -> [i64; 4] {
        rusqlite::Connection::open(self.store.path())
            .expect("database")
            .query_row(
                "SELECT (SELECT COUNT(*) FROM cloud_workflows), (SELECT COUNT(*) FROM remote_runtime_allocations),
             (SELECT COUNT(*) FROM cloud_worker_creation_claims), (SELECT COUNT(*) FROM remote_first_pin_intents)",
                [],
                |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?]),
            )
            .expect("counts")
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn public_apis_refuse_unsupported_platform_before_identity_or_provider_work() {
    let f = Fixture::new();
    let error = RunPodWorkspaceSetupError::from(RemoteSshIdentityError::UnsupportedPlatform);
    assert_eq!(
        start_task_free_runpod_workspace(&f.store, &f.identities, &f.key, &f.profile, &f.dormant, i64::MAX),
        Err(error)
    );
    assert_eq!(f.counts(), [0; 4]);
    assert_eq!(
        start_task_free_runpod_workspace_with_network_volume(
            &f.store,
            &f.identities,
            &f.key,
            &f.profile,
            &f.dormant,
            i64::MAX,
            &RunPodNetworkVolumeExpectation {
                volume_id: "volume".into(),
                data_center_id: "DC".into(),
                minimum_size_gb: 10
            }
        ),
        Err(RemoteSshIdentityError::UnsupportedPlatform.into())
    );
    assert_eq!(f.counts(), [0; 4]);
    let allocation = f
        .store
        .allocate_remote_runtime(&f.dormant, i64::MAX)
        .expect("fixture allocation");
    for operation in [retry_runpod_workspace_setup, recover_runpod_workspace] {
        assert_eq!(
            operation(&f.store, &f.identities, &f.key, &f.profile, &allocation),
            Err(RemoteSshIdentityError::UnsupportedPlatform.into())
        );
    }
    assert_eq!(f.counts(), [1, 1, 0, 0]);
    assert!(!f.directory.path().join("home/remote-ssh-identities").exists());
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq)]
struct Snapshot {
    allocation: StoredRemoteAllocation,
    counts: [i64; 4],
    keys: Vec<(std::ffi::OsString, ArtifactDigest)>,
}

#[cfg(target_os = "linux")]
impl Fixture {
    fn allocation(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn keys(&self) -> Vec<(std::ffi::OsString, ArtifactDigest)> {
        let path = self.directory.path().join("home/remote-ssh-identities");
        let mut files = match std::fs::read_dir(path) {
            Ok(entries) => entries
                .map(|entry| {
                    let entry = entry.expect("entry");
                    let digest = ArtifactDigest::sha256(&std::fs::read(entry.path()).expect("key bytes"));
                    (entry.file_name(), digest)
                })
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("fixture key read failed: {error}"),
        };
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            allocation: self.allocation(),
            counts: self.counts(),
            keys: self.keys(),
        }
    }

    fn reserve(&self, intent: bool) -> StoredRemoteAllocation {
        let allocation = self
            .store
            .allocate_remote_runtime(&self.dormant, i64::MAX)
            .expect("allocate");
        let allocation = prepare_identity(&self.store, &self.identities, &allocation).expect("reserve identity");
        if intent {
            self.store.record_remote_first_pin_intent(&allocation).expect("intent");
        }
        allocation
    }

    fn factory(&self, trust: &TrustSelection) -> Provider {
        let initial = matches!(trust, TrustSelection::Initial(_));
        self.remote.selections.lock().expect("selections")[usize::from(!initial)] += 1;
        Provider {
            store: self.store.clone(),
            remote: self.remote.clone(),
        }
    }

    fn start(&self) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
        self.start_using(i64::MAX, |trust| self.factory(&trust))
    }

    fn start_using(
        &self,
        retention: i64,
        factory: impl FnOnce(TrustSelection) -> Provider,
    ) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
        let Self {
            store,
            identities,
            key,
            profile,
            dormant,
            ..
        } = self;
        start_with(
            store,
            identities,
            key,
            profile,
            dormant,
            retention,
            |trust, _, selection| {
                assert!(selection.is_none(), "ordinary fixture has no saved selection");
                Ok(factory(trust))
            },
        )
    }

    fn run(&self, operation: Operation) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
        self.run_expected(&self.allocation(), operation, |trust| self.factory(&trust))
    }

    fn run_expected(
        &self,
        expected: &StoredRemoteAllocation,
        operation: Operation,
        factory: impl FnOnce(TrustSelection) -> Provider,
    ) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
        let Self {
            store,
            identities,
            key,
            profile,
            ..
        } = self;
        dispatch(store, identities, key, profile, expected, operation, factory)
    }

    fn assert_refused(&self, error: &RunPodWorkspaceSetupError) {
        let diagnostic = format!("{error:?} {error}");
        for secret in [
            "synthetic-private-credential",
            self.directory.path().to_str().expect("fixture path"),
        ] {
            assert!(!diagnostic.contains(secret));
        }
        let before = self.snapshot();
        for operation in [Operation::Retry, Operation::Recover] {
            assert_eq!(&self.run(operation).expect_err("refused"), error);
        }
        assert_eq!(self.snapshot(), before);
        assert_eq!(*self.remote.calls.lock().expect("calls"), [0; 5]);
        assert_eq!(*self.remote.selections.lock().expect("selections"), [0; 2]);
    }

    fn claim(&self, allocation: &StoredRemoteAllocation) {
        let r = allocation.worker_request().expect("request");
        assert!(
            self.store
                .claim_worker_creation(r.workflow_id, r.job_id, &r.target, "synthetic-worker")
                .expect("claim")
        );
    }

    fn status(&self) -> InteractiveWorkerStatus {
        status(&self.allocation().worker_request().expect("request"), false)
    }
}

// Keep ordinary lifecycle fixtures focused on trust mode; network tests exercise
// the request/selection-aware fallible factory directly.
#[cfg(target_os = "linux")]
fn dispatch(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    key: &RunPodApiKey,
    profile: &RunPodProfile,
    expected: &StoredRemoteAllocation,
    operation: Operation,
    factory: impl FnOnce(TrustSelection) -> Provider,
) -> Result<StoredRemoteAllocation, RunPodWorkspaceSetupError> {
    super::dispatch(
        store,
        identities,
        key,
        profile,
        expected,
        operation,
        |trust, _, selection| {
            assert!(selection.is_none(), "ordinary fixture has no saved selection");
            Ok(factory(trust))
        },
    )
}

#[cfg(target_os = "linux")]
fn record_management(store: &CloudWorkflowStore, expected: &StoredRemoteAllocation) {
    let mut state = expected.workspace().state().clone();
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    store
        .replace_remote_workspace(expected.workspace(), &state)
        .expect("management");
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Remote {
    calls: Mutex<[usize; 5]>,      // ensure, create, reconcile, inspect, delete
    selections: Mutex<[usize; 2]>, // initial, retained
    observation: Mutex<Option<InteractiveWorkerStatus>>,
    provisioning: AtomicBool,
    lose_response: AtomicBool,
    after_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[cfg(target_os = "linux")]
struct Provider {
    store: CloudWorkflowStore,
    remote: Arc<Remote>,
}

#[cfg(target_os = "linux")]
fn status(request: &InteractiveWorkerRequest, provisioning: bool) -> InteractiveWorkerStatus {
    InteractiveWorkerStatus {
        worker: InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "synthetic-worker".into(),
            },
            target: request.target.clone(),
            ssh_public_key: request.ssh_public_key.clone(),
            lifetime: InteractiveWorkerLifetime::Persistent,
        },
        lifecycle: if provisioning {
            InteractiveWorkerLifecycle::Provisioning
        } else {
            InteractiveWorkerLifecycle::Ready
        },
        ssh: (!provisioning).then(|| InteractiveWorkerSshEndpoint {
            host: "127.0.0.1".into(),
            port: 2222,
            username: "horizon".into(),
            host_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH".into(),
        }),
    }
}

#[cfg(target_os = "linux")]
impl Provider {
    fn read(&self, index: usize) -> Option<InteractiveWorkerStatus> {
        self.remote.calls.lock().expect("calls")[index] += 1;
        let value = self.remote.observation.lock().expect("observation").clone();
        if let Some(action) = self.remote.after_read.lock().expect("hook").take() {
            action();
        }
        value
    }
}

#[cfg(target_os = "linux")]
impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::RunPod
    }
    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        self.remote.calls.lock().expect("calls")[0] += 1;
        let created = self
            .store
            .claim_worker_creation(request.workflow_id, request.job_id, &request.target, "synthetic-worker")
            .map_err(|_| std::io::Error::other("private-fence-payload"))?;
        if created {
            self.remote.calls.lock().expect("calls")[1] += 1;
            *self.remote.observation.lock().expect("observation") =
                Some(status(request, self.remote.provisioning.load(Ordering::SeqCst)));
        }
        if self.remote.lose_response.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("private-provider-payload"));
        }
        let value = self
            .remote
            .observation
            .lock()
            .expect("observation")
            .clone()
            .ok_or_else(|| std::io::Error::other("unresolved"))?;
        Ok(if created {
            InteractiveWorkerEnsure::Created(value)
        } else {
            InteractiveWorkerEnsure::Reused(value)
        })
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(self.read(2))
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(self.read(3))
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.remote.calls.lock().expect("calls")[4] += 1;
        Err(std::io::Error::other("delete forbidden"))
    }
}
