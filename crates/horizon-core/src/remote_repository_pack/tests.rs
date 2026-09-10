use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, StoredRemoteAllocation, interactive_worker::*},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
    remote_workspace_recovery::recover_remote_workspace,
};

#[path = "tests/protocol.rs"]
mod wire;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

struct Expected {
    path: String,
    base: GitCommitSha,
    digest: ArtifactDigest,
    bytes: u64,
}

impl Expected {
    fn new() -> Self {
        Self {
            path: "/retained/private-candidate".into(),
            base: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            digest: ArtifactDigest::parse_sha256("a".repeat(64)).expect("digest"),
            bytes: 64,
        }
    }

    fn request(&self) -> RemotePackExpectation<'_> {
        RemotePackExpectation {
            path: &self.path,
            base_commit: &self.base,
            sha256: &self.digest,
            encoded_bytes: self.bytes,
        }
    }

    fn response(&self) -> serde_json::Value {
        serde_json::json!({
            "version": 1, "status": "observed", "retained": null, "reason": null,
            "pack": {"path": self.path, "objects_directory": format!("{}/decoded/objects", self.path),
                "identity": {"base_commit": self.base, "sha256": self.digest, "encoded_bytes": self.bytes},
                "objects": 3}
        })
    }

    fn response_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self.response()).expect("response")
    }
}

pub(crate) struct Fixture {
    pub(crate) directory: tempfile::TempDir,
    pub(crate) store: CloudWorkflowStore,
    pub(crate) recovered: RecoveredRemoteWorkspace,
}

impl Fixture {
    pub(crate) fn new(lifecycle: Option<InteractiveWorkerLifecycle>) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let identities = RemoteSshIdentityStore::new(&home);
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": {"provider": "local_docker", "profile": "development",
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent"},
                "repository": {"repository": "example/project", "commit": "b".repeat(40)}
            }
        }))
        .expect("state");
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        let identity = identities
            .prepare_new(runtime.workflow_id, runtime.job_id)
            .expect("identity");
        let allocation = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
            .expect("request");
        let request = allocation.worker_request().expect("request");
        let status = lifecycle.map(|lifecycle| InteractiveWorkerStatus {
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
            lifecycle,
            ssh: Some(InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "horizon".into(),
                host_key: identity.public_key().into(),
            }),
        });
        let recovered = recover_remote_workspace(&store, &identities, &Provider(status), OWNER, "workspace")
            .expect("fixture recovery");
        Self {
            directory,
            store,
            recovered,
        }
    }

    pub(crate) fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn check(
        &self,
        expected: &Expected,
        execute: impl FnOnce(&[u8]) -> Result<Vec<u8>, RemotePackInspectionError>,
    ) -> Result<RemotePackObservation, RemotePackInspectionError> {
        inspect_with(
            &self.store,
            &self.recovered,
            expected.request(),
            |identity, endpoint, input| {
                assert_eq!(identity.public_key(), self.recovered.identity().public_key());
                assert_eq!(
                    Some(endpoint),
                    self.recovered.observation().and_then(|status| status.ssh.as_ref())
                );
                execute(input)
            },
        )
    }

    pub(crate) fn edit(&self, stop: bool) {
        let current = self.current();
        if stop {
            self.store
                .record_remote_stop_phase(&current, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                .expect("stop intent");
        } else {
            let mut state = current.workspace().state().clone();
            state.spec.working_directory = "src".into();
            self.store
                .replace_remote_workspace(current.workspace(), &state)
                .expect("new intent");
        }
    }
}

struct Provider(Option<InteractiveWorkerStatus>);

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::LocalDocker
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(self.0.clone())
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(self.0.clone())
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

#[test]
fn zero_panel_workspace_queries_exact_pack_without_mutating_saved_state_or_identity() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let expected = Expected::new();
    let before = fixture.current();
    assert!(before.workspace().state().spec.panels.is_empty());
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
    let observation = fixture
        .check(&expected, |input| {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(input).expect("request"),
                serde_json::json!({
                    "version": 1, "path": expected.path,
                    "pack": {"base_commit": expected.base, "sha256": expected.digest, "encoded_bytes": expected.bytes}
                })
            );
            Ok(expected.response_bytes())
        })
        .expect("observation");
    assert_eq!(observation.path(), expected.path);
    assert_eq!(
        observation.objects_directory(),
        format!("{}/decoded/objects", expected.path)
    );
    assert_eq!(observation.base_commit(), &expected.base);
    assert_eq!(observation.sha256(), &expected.digest);
    assert_eq!(observation.encoded_bytes(), expected.bytes);
    assert_eq!(observation.objects(), 3);
    assert!(!format!("{observation:?}").contains("private-candidate"));
    assert_eq!(fixture.current(), before);
    assert_eq!(
        std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
        key
    );
}

#[test]
fn database_loss_before_or_during_query_rejects_observation_without_recreation() {
    for during_query in [false, true] {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let expected = Expected::new();
        let path = fixture.store.path();
        let retained = path.with_file_name("retained.sqlite3");
        let before = std::fs::read(path).expect("saved database");
        if !during_query {
            std::fs::rename(path, &retained).expect("retain database before query");
        }
        let result = fixture.check(&expected, |_| {
            assert!(during_query, "storage admission must precede SSH");
            std::fs::rename(path, &retained).expect("retain database during query");
            Ok(expected.response_bytes())
        });
        assert!(matches!(
            result,
            Err(RemotePackInspectionError::Admission(
                RemotePanelStatusError::StorageUnavailable
            ))
        ));
        assert!(!path.exists(), "query must not recreate the database");
        assert_eq!(std::fs::read(retained).expect("preserved database"), before);
    }
}

#[test]
fn wrong_base_missing_or_nonready_worker_never_crosses_ssh() {
    let mut expected = Expected::new();
    for lifecycle in [None, Some(InteractiveWorkerLifecycle::Stopped)] {
        let fixture = Fixture::new(lifecycle);
        assert!(matches!(
            fixture.check(&expected, |_| panic!("no SSH")),
            Err(RemotePackInspectionError::Admission(
                RemotePanelStatusError::WorkerUnavailable
            ))
        ));
    }
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    expected.base = GitCommitSha::parse("c".repeat(40)).expect("other base");
    assert!(matches!(
        fixture.check(&expected, |_| panic!("no SSH")),
        Err(RemotePackInspectionError::InvalidRequest)
    ));
}

#[test]
fn snapshot_or_management_drift_before_and_during_query_rejects_late_results() {
    let expected = Expected::new();
    for stop in [false, true] {
        for during_query in [false, true] {
            let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
            if !during_query {
                fixture.edit(stop);
            }
            assert!(matches!(
                fixture.check(&expected, |_| {
                    assert!(during_query, "stale request must reject before SSH");
                    fixture.edit(stop);
                    Ok(expected.response_bytes())
                }),
                Err(RemotePackInspectionError::Admission(
                    RemotePanelStatusError::StateChanged
                ))
            ));
            assert_ne!(fixture.current(), *fixture.recovered.allocation());
        }
    }
}

#[test]
fn query_failures_preserve_the_candidate_and_do_not_invent_absence_or_replay_authority() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let expected = Expected::new();
    let before = fixture.current();
    for error in [
        RemotePackInspectionError::QueryFailed,
        RemotePackInspectionError::Deadline,
        RemotePackInspectionError::InvalidResponse,
    ] {
        let diagnostic = error.to_string();
        let result = fixture.check(&expected, |_| Err(error)).expect_err("query failure");
        assert_eq!(result.to_string(), diagnostic);
        assert!(!diagnostic.contains("private-candidate"));
        assert_eq!(fixture.current(), before);
    }
}
