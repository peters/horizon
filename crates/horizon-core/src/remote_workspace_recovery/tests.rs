use super::*;
use crate::{
    HorizonHome,
    cloud_run::{
        CloudProvider, WorkerLifetime,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerRequest,
            InteractiveWorkerSshEndpoint,
        },
    },
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

mod observations;
#[cfg(target_os = "linux")]
mod retained_identity;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn public_key(byte: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
}

impl Fixture {
    fn new() -> Self {
        Self::with_lifetime(WorkerLifetime::Persistent)
    }

    fn with_lifetime(lifetime: WorkerLifetime) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        }
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": { "provider": "local_docker", "profile": "development",
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent" },
                "repository": { "repository": "example/project", "commit": "b".repeat(40) }
            }
        }))
        .expect("state");
        state.spec.target.lifetime = lifetime;
        let dormant = store.create_remote_workspace(OWNER, &state).expect("record");
        store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        Self {
            directory,
            store,
            identities: RemoteSshIdentityStore::new(&home),
        }
    }

    fn reload(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn reserve(&self, key: &str) {
        self.store
            .reserve_remote_worker_request(&self.reload(), key)
            .expect("reserve");
    }

    fn status(&self) -> InteractiveWorkerStatus {
        let request = self.reload().worker_request().expect("request");
        InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: request.target.provider,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "synthetic-worker".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle: InteractiveWorkerLifecycle::Ready,
            ssh: Some(InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "horizon".into(),
                host_key: public_key(7),
            }),
        }
    }

    fn run(&self, provider: &Provider) -> Result<RecoveredRemoteWorkspace, RemoteWorkspaceRecoveryError> {
        recover_remote_workspace(&self.store, &self.identities, provider, OWNER, "workspace")
    }

    fn cancel(&self) {
        let current = self.reload();
        let mut next = current.workspace().state().clone();
        let runtime = next.runtime.as_mut().expect("runtime");
        runtime.phase = RemoteRuntimePhase::Cancelling;
        runtime.cleanup = Some(RemoteCleanupIntent {
            reason: RemoteCleanupReason::Cancelled,
            requested_at_millis: 1,
        });
        self.store
            .replace_remote_workspace(current.workspace(), &next)
            .expect("cancel intent");
    }

    fn counts(&self) -> [i64; 3] {
        rusqlite::Connection::open(self.store.path())
            .expect("connection")
            .query_row(
                "SELECT (SELECT COUNT(*) FROM cloud_workflows), (SELECT COUNT(*) FROM remote_runtime_allocations),
             (SELECT COUNT(*) FROM cloud_worker_creation_claims)",
                [],
                |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?]),
            )
            .expect("counts")
    }
}

struct Provider {
    kind: CloudProvider,
    status: Option<InteractiveWorkerStatus>,
    calls: Mutex<[usize; 4]>,
    during_read: Option<Box<dyn Fn() + Send + Sync>>,
    fail: bool,
}

impl Provider {
    fn new(status: Option<InteractiveWorkerStatus>) -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            status,
            calls: Mutex::new([0; 4]),
            during_read: None,
            fail: false,
        }
    }

    fn calls(&self) -> [usize; 4] {
        *self.calls.lock().expect("calls")
    }

    fn read(&self, index: usize) -> Result<Option<InteractiveWorkerStatus>, std::io::Error> {
        self.calls.lock().expect("calls")[index] += 1;
        if let Some(action) = &self.during_read {
            action();
        }
        if self.fail {
            return Err(std::io::Error::other("synthetic-private-provider-response"));
        }
        Ok(self.status.clone())
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        self.calls.lock().expect("calls")[0] += 1;
        Err(std::io::Error::other("creation forbidden"))
    }
    fn reconcile_worker(
        &self,
        request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        if let Some(status) = &self.status {
            assert_eq!(request.workflow_id, status.worker.identity.workflow_id);
        }
        self.read(1)
    }
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        assert_eq!(worker.identity.resource_id, "synthetic-worker");
        self.read(2)
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.calls.lock().expect("calls")[3] += 1;
        Err(std::io::Error::other("deletion forbidden"))
    }
}

#[test]
fn missing_request_wrong_owner_provider_and_management_fail_before_provider_io() {
    let fixture = Fixture::new();
    let mut provider = Provider::new(None);
    assert_eq!(
        fixture.run(&provider).expect_err("request"),
        RemoteWorkspaceRecoveryError::MissingRequest
    );
    fixture.reserve(&public_key(1));
    let saved = fixture.reload();
    assert_eq!(
        recover_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            "00000000-0000-4000-8000-000000000002",
            "workspace"
        )
        .expect_err("owner"),
        RemoteWorkspaceRecoveryError::StorageUnavailable
    );
    assert_eq!(
        recover_remote_workspace(&fixture.store, &fixture.identities, &provider, OWNER, "absent").expect_err("absent"),
        RemoteWorkspaceRecoveryError::MissingAllocation
    );
    provider.kind = CloudProvider::RunPod;
    assert_eq!(
        fixture.run(&provider).expect_err("provider"),
        RemoteWorkspaceRecoveryError::ProviderMismatch
    );
    assert_eq!(fixture.reload(), saved);
    fixture.cancel();
    assert_eq!(
        fixture.run(&provider).expect_err("management"),
        RemoteWorkspaceRecoveryError::ManagementPending
    );
    assert_eq!(provider.calls(), [0; 4]);
    assert_eq!(fixture.counts(), [1, 1, 0]);
    assert!(fixture.directory.path().exists());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_private_identity_platform_fails_before_provider_io() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let saved = fixture.reload();
    let provider = Provider::new(None);
    assert_eq!(
        fixture.run(&provider).expect_err("unsupported"),
        RemoteWorkspaceRecoveryError::Identity(RemoteSshIdentityError::UnsupportedPlatform)
    );
    assert_eq!(provider.calls(), [0; 4]);
    assert_eq!(fixture.reload(), saved);
}
