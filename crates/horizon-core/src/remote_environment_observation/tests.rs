use super::*;
use crate::{
    cloud_run::{
        ArtifactDigest, CloudProvider, WorkerLifetime,
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerLease,
            InteractiveWorkerLifetime, InteractiveWorkerRequest, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
        },
    },
    remote_workspace::{
        RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState, RepositoryCheckpoint,
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

mod configured;
mod guards;
#[cfg(target_os = "linux")]
mod runpod;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
type Error = RemoteEnvironmentObservationError;

fn public_key(byte: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
}

impl Fixture {
    fn new() -> Self {
        Self::with_request(WorkerLifetime::Persistent, true)
    }

    fn with_request(lifetime: WorkerLifetime, reserve: bool) -> Self {
        Self::with_provider(lifetime, reserve, CloudProvider::LocalDocker, false)
    }

    fn with_provider(lifetime: WorkerLifetime, reserve: bool, provider: CloudProvider, network: bool) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0,
                "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)},
                "panels":[{"panel_local_id":"terminal", "kind":"command",
                    "command":{"program":"printf", "args":["private-task-marker"]}}]
            }
        }))
        .expect("state");
        state.spec.target.lifetime = lifetime;
        state.spec.target.provider = provider;
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        if network {
            store
                .record_remote_network_volume_selection(
                    &allocation,
                    &crate::cloud_run::runpod::RunPodNetworkVolumeExpectation {
                        volume_id: "synthetic-volume".into(),
                        data_center_id: "synthetic-dc".into(),
                        minimum_size_gb: 10,
                    },
                )
                .expect("selection");
        }
        if reserve {
            // Synthetic public identity only: overview observation must not open a private key.
            store
                .reserve_remote_worker_request(&allocation, &public_key(1))
                .expect("request");
        }
        Self {
            _directory: directory,
            store,
        }
    }

    fn reload(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn status(&self) -> InteractiveWorkerStatus {
        let request = self.reload().worker_request().expect("request");
        let lifetime = match request.target.lifetime {
            WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
            WorkerLifetime::TimeLimited { seconds } => InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(seconds)))
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("deadline"),
            }),
        };
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
                lifetime,
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

    fn retain(&self, status: &InteractiveWorkerStatus) {
        self.store
            .record_remote_worker_recovery(&self.reload(), Some(status))
            .expect("retained observation");
    }

    fn observe(&self, provider: &Provider) -> Result<RemoteEnvironmentObservation, Error> {
        observe_remote_environment(&self.store, provider, self.reload().workspace())
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
}

struct Provider {
    kind: CloudProvider,
    status: Option<InteractiveWorkerStatus>,
    calls: Mutex<Vec<&'static str>>,
    during_read: Option<Box<dyn Fn() + Send + Sync>>,
    fail: bool,
}

impl Provider {
    fn new(status: Option<InteractiveWorkerStatus>) -> Self {
        Self {
            kind: CloudProvider::LocalDocker,
            status,
            calls: Mutex::default(),
            during_read: None,
            fail: false,
        }
    }

    fn read(&self, operation: &'static str) -> Result<Option<InteractiveWorkerStatus>, std::io::Error> {
        self.calls.lock().expect("calls").push(operation);
        if let Some(action) = &self.during_read {
            action();
        }
        if self.fail {
            return Err(std::io::Error::other("private-provider-payload"));
        }
        Ok(self.status.clone())
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls").clone()
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        self.calls.lock().expect("calls").push("ensure");
        Err(std::io::Error::other("forbidden create"))
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.read("reconcile")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.read("inspect")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.calls.lock().expect("calls").push("delete");
        Err(std::io::Error::other("forbidden delete"))
    }
}

#[test]
fn public_request_observation_needs_no_private_key_and_never_adopts_the_worker() {
    let fixture = Fixture::new();
    let status = fixture.status();
    let provider = Provider::new(Some(status.clone()));
    let before = fixture.reload();
    let result = fixture.observe(&provider).expect("observation");
    assert_eq!(result.saved, before.workspace().environment_summary());
    assert!(result.saved.worker_identity.is_none());
    assert_eq!(
        result.worker,
        Some(ObservedRemoteWorker {
            identity: status.worker.identity,
            lifecycle: status.lifecycle
        })
    );
    assert!(result.observed_at_millis > 0);
    assert_eq!(provider.calls(), ["reconcile"]);
    assert_eq!(fixture.reload(), before);
    assert_eq!(fixture.counts(), [1, 1, 0]);
    let debug = format!("{result:?}");
    for secret in ["private-task-marker", "127.0.0.1", "ssh-ed25519"] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn retained_identity_lifecycle_and_absence_do_not_change_saved_state() {
    let fixture = Fixture::new();
    let retained = fixture.status();
    fixture.retain(&retained);
    let before = fixture.reload();
    for lifecycle in [
        InteractiveWorkerLifecycle::Provisioning,
        InteractiveWorkerLifecycle::Ready,
        InteractiveWorkerLifecycle::Stopped,
        InteractiveWorkerLifecycle::Failed,
        InteractiveWorkerLifecycle::Deleting,
        InteractiveWorkerLifecycle::Unknown,
    ] {
        let mut status = retained.clone();
        status.lifecycle = lifecycle;
        if lifecycle != InteractiveWorkerLifecycle::Ready {
            status.ssh = None;
        }
        let provider = Provider::new(Some(status));
        let result = fixture.observe(&provider).expect("observation");
        assert_eq!(result.worker.expect("observed worker").lifecycle, lifecycle);
        assert_eq!(provider.calls(), ["inspect"]);
        assert_eq!(fixture.reload(), before);
    }
    let provider = Provider::new(None);
    let result = fixture.observe(&provider).expect("absence");
    assert!(result.worker.is_none());
    assert_eq!(result.saved.worker_identity, Some(retained.worker.identity));
    assert_eq!(provider.calls(), ["inspect"]);
    assert_eq!(fixture.reload(), before);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn pending_management_and_checkpoint_are_observable_but_never_mutated() {
    let fixture = Fixture::new();
    let status = fixture.status();
    fixture.retain(&status);
    for phase in [RemoteRuntimePhase::Cancelling, RemoteRuntimePhase::Deleting] {
        let before = fixture.reload();
        let mut state = before.workspace().state().clone();
        let runtime = state.runtime.as_mut().expect("runtime");
        runtime.phase = phase;
        runtime.cleanup = Some(RemoteCleanupIntent {
            reason: RemoteCleanupReason::Cancelled,
            requested_at_millis: 1,
        });
        state.checkpoint = Some(RepositoryCheckpoint {
            workspace_local_id: "workspace".into(),
            base_commit: state.spec.repository.commit.clone(),
            manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
            runtime_generation: 1,
            generation: 1,
            captured_at_millis: 1,
            recovery_artifact: None,
        });
        fixture
            .store
            .replace_remote_workspace(before.workspace(), &state)
            .expect("management");
        let expected = fixture.reload();
        let provider = Provider::new(Some(status.clone()));
        let result = fixture.observe(&provider).expect("read-only observation");
        assert_eq!(result.saved.saved_phase, Some(phase));
        assert_eq!(result.saved.checkpoint, state.checkpoint);
        assert_eq!(fixture.reload(), expected);
        assert_eq!(provider.calls(), ["inspect"]);
        assert!(
            fixture
                .store
                .record_remote_worker_recovery(&expected, Some(&status))
                .is_err()
        );
    }
    assert_eq!(fixture.counts(), [1, 1, 0]);
}
