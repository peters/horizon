use super::*;
use crate::{
    HorizonHome, PanelKind,
    cloud_run::{CloudProvider, StoredRemoteAllocation, interactive_worker::*},
    remote_ssh_identity::RemoteSshIdentityStore,
    remote_workspace::{RemotePanelBinding, RemoteWorkspaceState},
    remote_workspace_recovery::recover_remote_workspace,
};

mod configured;
mod runpod;

#[path = "tests/intent.rs"]
mod intent_tests;
#[path = "tests/ssh.rs"]
mod ssh_tests;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const RUNNING: &[u8] = br#"{"state":"running","panel":"terminal","pid":123,"exit_status":null}"#;

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    recovered: RecoveredRemoteWorkspace,
}

impl Fixture {
    fn new() -> Self {
        Self::with_lifecycle(InteractiveWorkerLifecycle::Ready)
    }

    fn with_lifecycle(lifecycle: InteractiveWorkerLifecycle) -> Self {
        Self::with_provider(lifecycle, CloudProvider::LocalDocker, None)
    }

    fn with_provider(
        lifecycle: InteractiveWorkerLifecycle,
        provider: CloudProvider,
        selection: Option<&crate::cloud_run::runpod::RunPodNetworkVolumeExpectation>,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private fixture");
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let identities = RemoteSshIdentityStore::new(&home);
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": { "provider": provider, "profile": "development",
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent" },
                "repository": { "repository": "example/project", "commit": "b".repeat(40) }
            }
        }))
        .expect("state");
        state.spec.panels.push(RemotePanelBinding {
            panel_local_id: "terminal".into(),
            kind: PanelKind::Shell,
            command: None,
            working_directory: None,
            task_handoff: None,
            agent_session_id: None,
        });
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        if let Some(selection) = selection {
            store
                .record_remote_network_volume_selection(&allocation, selection)
                .expect("selection before identity or claim");
        }
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        let identity = identities
            .prepare_new(runtime.workflow_id, runtime.job_id)
            .expect("identity");
        let allocation = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
            .expect("request");
        let request = allocation.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
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
        };
        let recovered = recover_remote_workspace(&store, &identities, &Provider(status), OWNER, "workspace")
            .expect("noncreating recovery");
        Self {
            directory,
            store,
            recovered,
        }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn edit_intent(&self) {
        let current = self.current();
        let mut next = current.workspace().state().clone();
        next.spec.working_directory = "src".into();
        self.store
            .replace_remote_workspace(current.workspace(), &next)
            .expect("edit");
    }
}

struct Provider(InteractiveWorkerStatus);

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.0.worker.identity.provider
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("fixture must never create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(Some(self.0.clone()))
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(Some(self.0.clone()))
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("fixture must never delete")
    }
}

#[test]
fn owned_status_is_noncreating_and_leaves_all_snapshots_and_private_identity_unchanged() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
    let result = inspect_with(
        &fixture.store,
        &fixture.recovered,
        "terminal",
        Inspection::Status,
        |identity, endpoint, input| {
            assert_eq!(
                identity.public_key(),
                before.worker_request().expect("request").ssh_public_key
            );
            assert_eq!(
                endpoint,
                fixture
                    .recovered
                    .observation()
                    .expect("observation")
                    .ssh
                    .as_ref()
                    .expect("SSH")
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(input).expect("request"),
                serde_json::json!({
                    "version": 1, "operation": "status", "runtime": before.worker_request().expect("request").job_id,
                    "panel": "terminal"
                })
            );
            Ok(RUNNING.to_vec())
        },
    );
    assert_eq!(result, Ok(RemotePanelStatus::Running { pid: 123 }));
    assert_eq!(fixture.current(), before);
    assert_eq!(
        std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
        key
    );
}

#[test]
fn unknown_panels_and_stale_ownership_never_cross_the_ssh_boundary() {
    let fixture = Fixture::new();
    for panel in ["absent", "terminal;start", ""] {
        assert_eq!(
            inspect_with(
                &fixture.store,
                &fixture.recovered,
                panel,
                Inspection::Status,
                |_, _, _| panic!("no SSH")
            ),
            Err(RemotePanelStatusError::UnknownPanel)
        );
    }
    fixture.edit_intent();
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::Status,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::StateChanged)
    );
}

#[test]
fn a_nonready_worker_cannot_be_queried_or_started() {
    let fixture = Fixture::with_lifecycle(InteractiveWorkerLifecycle::Stopped);
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::Status,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::WorkerUnavailable)
    );
}

#[test]
fn late_status_cannot_hide_newer_management_or_workspace_intent() {
    let fixture = Fixture::new();
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::Status,
            |_, _, _| {
                fixture.edit_intent();
                Ok(RUNNING.to_vec())
            }
        ),
        Err(RemotePanelStatusError::StateChanged)
    );
    assert_eq!(fixture.current().workspace().state().spec.working_directory, "src");
}

#[test]
fn status_protocol_preserves_unknown_completion_and_rejects_wrong_or_unbounded_results() {
    assert_eq!(
        protocol::response(br#"{"state":"unavailable","panel":"terminal"}"#, "terminal"),
        Ok(RemotePanelStatus::Unavailable)
    );
    for exit_status in [None, Some(0), Some(42)] {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"state":"exited","panel":"terminal","pid":123,"exit_status":exit_status}),
        )
        .expect("json");
        assert_eq!(
            protocol::response(&bytes, "terminal"),
            Ok(RemotePanelStatus::Exited { pid: 123, exit_status })
        );
    }
    for bytes in [
        b"synthetic-private-response".as_slice(),
        br#"{"state":"running","panel":"terminal","pid":123}"#,
        br#"{"state":"exited","panel":"terminal","pid":123}"#,
        br#"{"state":"running","panel":"other","pid":123}"#,
        br#"{"state":"running","panel":"terminal","pid":0}"#,
        br#"{"state":"running","panel":"terminal","pid":123,"exit_status":0}"#,
        br#"{"state":"exited","panel":"terminal","pid":123,"exit_status":256}"#,
        br#"{"state":"unavailable","panel":"terminal","argv":["secret"]}"#,
        br#"{"state":"running","panel":"terminal","panel":"other","pid":123}"#,
    ] {
        assert_eq!(
            protocol::response(bytes, "terminal"),
            Err(RemotePanelStatusError::InvalidResponse)
        );
    }
    assert_eq!(
        protocol::response(&vec![b' '; protocol::RESPONSE_LIMIT + 1], "terminal"),
        Err(RemotePanelStatusError::InvalidResponse)
    );
}
