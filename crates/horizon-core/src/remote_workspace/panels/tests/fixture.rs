use super::*;
use crate::cloud_run::interactive_worker::{
    InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLease, InteractiveWorkerLifecycle,
    InteractiveWorkerLifetime, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(super) const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn public_key(byte: u8) -> String {
    let mut bytes = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    bytes.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(bytes))
}

pub(super) struct Fixture {
    pub directory: tempfile::TempDir,
    pub store: CloudWorkflowStore,
}

impl Fixture {
    pub fn ready() -> Self {
        Self::new(CloudProvider::LocalDocker, WorkerLifetime::Persistent, true)
    }

    pub fn new(provider: CloudProvider, lifetime: WorkerLifetime, pin: bool) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":"nested", "generation":0,
                "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository":{"repository":"example/project", "commit":"b".repeat(40), "branch":"work/example"},
                "panels":[{"panel_local_id":"original", "kind":"shell",
                    "command":{"program":"/bin/sh", "args":["-c", "printf original"]}}]
            }
        }))
        .expect("state");
        state.spec.target.provider = provider;
        state.spec.target.lifetime = lifetime;
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        let allocation = store
            .reserve_remote_worker_request(&allocation, &public_key(1))
            .expect("public identity");
        let request = allocation.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "synthetic-worker".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: match lifetime {
                    WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
                    WorkerLifetime::TimeLimited { .. } => {
                        InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                            terminate_after: "2099-01-01T00:00:00Z".into(),
                        })
                    }
                },
            },
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: pin.then(|| InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "horizon".into(),
                host_key: public_key(7),
            }),
        };
        store
            .record_remote_worker_recovery(&allocation, Some(&status))
            .expect("saved observation");
        Self { directory, store }
    }

    pub fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    pub fn summary(&self) -> RemoteEnvironmentSummary {
        self.current().workspace().environment_summary()
    }

    pub fn prepare(&self) -> PreparedRemoteShellPanel {
        prepare_remote_shell_panel(&self.store, OWNER, &self.summary(), draft()).expect("preview")
    }

    pub fn edit(&self, change: impl FnOnce(&mut RemoteWorkspaceState)) {
        let allocation = self.current();
        let mut state = allocation.workspace().state().clone();
        change(&mut state);
        self.store
            .replace_remote_workspace(allocation.workspace(), &state)
            .expect("edit");
    }

    pub fn counts(&self) -> [i64; 3] {
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
}

pub(super) fn draft() -> RemoteShellPanelDraft {
    RemoteShellPanelDraft {
        command: RemotePanelCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf independent".into()],
        },
        working_directory: Some("nested/second".into()),
    }
}
