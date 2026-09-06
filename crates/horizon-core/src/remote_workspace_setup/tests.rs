use super::*;
use crate::{
    HorizonHome,
    cloud_run::{
        CloudProvider,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerRequest,
            InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
        },
    },
    remote_workspace::RemoteWorkspaceState,
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cfg(target_os = "linux")]
mod lifecycle;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    dormant: StoredRemoteWorkspace,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        }
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
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
        let dormant = store.create_remote_workspace(OWNER, &state).expect("record");
        Self {
            directory,
            store,
            identities: RemoteSshIdentityStore::new(&home),
            dormant,
        }
    }

    fn provider(&self) -> Provider {
        Provider {
            kind: CloudProvider::LocalDocker,
            store: self.store.clone(),
            identities: RemoteSshIdentityStore::new(&HorizonHome::from_root(self.directory.path().join("home"))),
            observation: Mutex::new(None),
            calls: Mutex::new([0; 5]),
            fail_response: AtomicBool::new(false),
            invalid_observation: false,
            after_create: None,
        }
    }

    fn counts(&self) -> [i64; 3] {
        rusqlite::Connection::open(self.store.path())
            .expect("database")
            .query_row(
                "SELECT (SELECT COUNT(*) FROM cloud_workflows), (SELECT COUNT(*) FROM remote_runtime_allocations),
             (SELECT COUNT(*) FROM cloud_worker_creation_claims)",
                [],
                |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?]),
            )
            .expect("counts")
    }

    #[cfg(target_os = "linux")]
    fn reload(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    #[cfg(target_os = "linux")]
    fn allocate(&self) -> StoredRemoteAllocation {
        self.store
            .allocate_remote_runtime(&self.dormant, i64::MAX)
            .expect("allocate")
    }

    #[cfg(target_os = "linux")]
    fn retry(&self, provider: &Provider) -> Result<StoredRemoteAllocation, RemoteWorkspaceSetupError> {
        retry_remote_workspace_setup(&self.store, &self.identities, provider, &self.reload())
    }
}

struct Provider {
    kind: CloudProvider,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    observation: Mutex<Option<InteractiveWorkerStatus>>,
    // ensure, create, reconcile, inspect, delete
    calls: Mutex<[usize; 5]>,
    fail_response: AtomicBool,
    invalid_observation: bool,
    after_create: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Provider {
    fn calls(&self) -> [usize; 5] {
        *self.calls.lock().expect("calls")
    }

    fn read(&self, index: usize) -> Option<InteractiveWorkerStatus> {
        self.calls.lock().expect("calls")[index] += 1;
        self.observation.lock().expect("observation").clone()
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }

    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        let allocation = self
            .store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("saved");
        assert_eq!(allocation.worker_request().expect("durable public request"), *request);
        self.identities
            .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
            .expect("private identity retained before provider I/O");
        self.calls.lock().expect("calls")[0] += 1;
        let granted = self
            .store
            .claim_worker_creation(request.workflow_id, request.job_id, &request.target, "synthetic-worker")
            .map_err(|_| std::io::Error::other("synthetic-private-store-failure"))?;
        if granted {
            self.calls.lock().expect("calls")[1] += 1;
            *self.observation.lock().expect("observation") = Some(InteractiveWorkerStatus {
                worker: InteractiveWorker {
                    identity: InteractiveWorkerIdentity {
                        provider: request.target.provider,
                        workflow_id: request.workflow_id,
                        job_id: request.job_id,
                        resource_id: "synthetic-worker".into(),
                    },
                    target: request.target.clone(),
                    ssh_public_key: request.ssh_public_key.clone(),
                    lifetime: InteractiveWorkerLifetime::Persistent,
                },
                lifecycle: InteractiveWorkerLifecycle::Ready,
                ssh: Some(InteractiveWorkerSshEndpoint {
                    host: "127.0.0.1".into(),
                    port: 2222,
                    username: "horizon".into(),
                    host_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH".into(),
                }),
            });
            if let Some(action) = &self.after_create {
                action();
            }
        }
        if self.fail_response.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("synthetic-private-provider-response"));
        }
        let mut status = self
            .observation
            .lock()
            .expect("observation")
            .clone()
            .ok_or_else(|| std::io::Error::other("reconciliation required"))?;
        if self.invalid_observation {
            status.worker.identity.job_id = crate::cloud_run::CloudJobId::new();
        }
        Ok(if granted {
            InteractiveWorkerEnsure::Created(status)
        } else {
            InteractiveWorkerEnsure::Reused(status)
        })
    }

    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(self.read(2))
    }

    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        assert_eq!(worker.identity.resource_id, "synthetic-worker");
        Ok(self.read(3))
    }

    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.calls.lock().expect("calls")[4] += 1;
        Err(std::io::Error::other("deletion forbidden"))
    }
}

#[test]
fn unsupported_platform_or_wrong_provider_cannot_allocate_or_call_provider() {
    let fixture = Fixture::new();
    let mut provider = fixture.provider();
    provider.kind = CloudProvider::RunPod;
    let expected = if cfg!(target_os = "linux") {
        RemoteWorkspaceRecoveryError::ProviderMismatch.into()
    } else {
        RemoteSshIdentityError::UnsupportedPlatform.into()
    };
    assert_eq!(
        start_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            &fixture.dormant,
            i64::MAX
        ),
        Err(expected)
    );
    assert_eq!(fixture.counts(), [0; 3]);
    assert_eq!(provider.calls(), [0; 5]);
}
