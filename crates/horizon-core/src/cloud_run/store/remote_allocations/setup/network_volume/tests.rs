use super::*;
use crate::cloud_run::interactive_worker::{
    InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLifecycle, InteractiveWorkerLifetime,
    InteractiveWorkerStatus,
};
use crate::cloud_run::store::encode_workflow;
use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{Arc, Barrier};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    saved: StoredRemoteAllocation,
}

impl Fixture {
    fn new() -> Self {
        Self::with_provider("run_pod")
    }
    fn with_provider(provider: &str) -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1, "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": { "provider": provider, "profile": "development",
                    "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent" },
                "repository": { "repository": "example/project", "commit": "b".repeat(40) }
            }
        }))
        .expect("state");
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let saved = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        Self {
            _directory: directory,
            store,
            saved,
        }
    }
    fn raw(&self) -> Connection {
        Connection::open(self.store.path()).expect("raw fixture")
    }
    fn reload(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }
    fn count(&self) -> i64 {
        self.raw()
            .query_row("SELECT COUNT(*) FROM remote_network_volume_selections", [], |row| {
                row.get(0)
            })
            .expect("count")
    }
    fn snapshots(&self) -> Vec<(i64, Vec<u8>)> {
        self.raw().prepare("SELECT revision, snapshot FROM remote_workspaces UNION ALL SELECT revision, snapshot FROM cloud_workflows")
            .expect("query").query_map([], |row| Ok((row.get(0)?, row.get(1)?))).expect("rows")
            .collect::<Result<_, _>>().expect("snapshots")
    }
    fn claim(&self) -> bool {
        let runtime = self.saved.workspace().state().runtime.as_ref().expect("runtime");
        self.store
            .claim_worker_creation(
                runtime.workflow_id,
                runtime.job_id,
                &self.saved.workspace().state().spec.target,
                "synthetic-worker",
            )
            .expect("claim")
    }
    fn reserve(&self) -> StoredRemoteAllocation {
        let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        key.extend([7; 32]);
        self.store
            .reserve_remote_worker_request(&self.reload(), &format!("ssh-ed25519 {}", STANDARD.encode(key)))
            .expect("key")
    }
    fn expire(&self) -> StoredRemoteAllocation {
        let mut workflow = self.reload().workflow().workflow().clone();
        workflow.created_at_millis = 1000;
        workflow.updated_at_millis = 1000;
        workflow.retain_until_millis = 2000;
        self.raw()
            .execute(
                "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
            retain_until_millis=2000, snapshot=?1",
                [encode_workflow(&workflow).expect("snapshot")],
            )
            .expect("expire");
        self.reload()
    }
    fn observe(&self) -> StoredRemoteAllocation {
        let saved = self.reserve();
        let request = saved.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::RunPod,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "synthetic-worker".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: None,
        };
        self.store
            .record_remote_worker_recovery(&saved, Some(&status))
            .expect("observation")
    }
}

fn selection() -> RunPodNetworkVolumeExpectation {
    RunPodNetworkVolumeExpectation {
        volume_id: "volume_exact".into(),
        data_center_id: "EU-TEST-1".into(),
        minimum_size_gb: 10,
    }
}

#[test]
fn immutable_selection_reopens_without_snapshot_changes_or_creation_grants() {
    let fixture = Fixture::new();
    let before = fixture.snapshots();
    assert_eq!(
        fixture
            .store
            .load_remote_network_volume_selection(&fixture.saved)
            .expect("unselected"),
        None
    );
    for _ in 0..2 {
        fixture
            .store
            .record_remote_network_volume_selection(&fixture.saved, &selection())
            .expect("record");
    }
    assert_eq!(fixture.count(), 1);
    assert_eq!(fixture.snapshots(), before);
    assert_eq!(fixture.reload(), fixture.saved);
    let reopened = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    assert_eq!(
        reopened
            .load_remote_network_volume_selection(&fixture.saved)
            .expect("read"),
        Some(selection())
    );
    assert!(fixture.claim());
    assert!(!fixture.claim());
    fixture
        .store
        .record_remote_network_volume_selection(&fixture.saved, &selection())
        .expect("same after claim");
    let expired = fixture.expire();
    let before = fixture.snapshots();
    fixture
        .store
        .record_remote_network_volume_selection(&expired, &selection())
        .expect("same after expiry");
    assert_eq!(
        reopened
            .load_remote_network_volume_selection(&expired)
            .expect("expired read"),
        Some(selection())
    );
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn different_selection_and_stale_or_foreign_allocations_cannot_replace_intent() {
    let fixture = Fixture::new();
    fixture
        .store
        .record_remote_network_volume_selection(&fixture.saved, &selection())
        .expect("record");
    for next in [
        RunPodNetworkVolumeExpectation {
            volume_id: "different".into(),
            ..selection()
        },
        RunPodNetworkVolumeExpectation {
            data_center_id: "OTHER-DC".into(),
            ..selection()
        },
        RunPodNetworkVolumeExpectation {
            minimum_size_gb: 11,
            ..selection()
        },
    ] {
        assert!(matches!(
            fixture
                .store
                .record_remote_network_volume_selection(&fixture.saved, &next),
            Err(Error::ReplacementIdentityMismatch)
        ));
    }
    let foreign = Fixture::new();
    assert!(
        fixture
            .store
            .load_remote_network_volume_selection(&foreign.saved)
            .is_err()
    );
    assert!(
        fixture
            .store
            .record_remote_network_volume_selection(&foreign.saved, &selection())
            .is_err()
    );
    let current = fixture.reserve();
    assert!(matches!(
        fixture.store.load_remote_network_volume_selection(&fixture.saved),
        Err(Error::SnapshotConflict)
    ));
    assert!(matches!(
        fixture
            .store
            .record_remote_network_volume_selection(&fixture.saved, &selection()),
        Err(Error::SnapshotConflict)
    ));
    assert_eq!(
        fixture
            .store
            .load_remote_network_volume_selection(&current)
            .expect("current"),
        Some(selection())
    );
    assert_eq!(fixture.count(), 1);
}

#[test]
fn first_record_refuses_claim_expiry_first_pin_observation_and_management_intent() {
    for mode in 0..7 {
        let fixture = Fixture::new();
        let mut saved = fixture.saved.clone();
        match mode {
            0 => {
                assert!(fixture.claim());
            }
            1 => {
                saved = fixture.expire();
            }
            2 => {
                saved = fixture.reserve();
                fixture.store.record_remote_first_pin_intent(&saved).expect("first pin");
            }
            3 => {
                saved = fixture.observe();
            }
            4 => {
                saved = fixture
                    .store
                    .record_remote_stop_phase(
                        &fixture.observe(),
                        RemoteRuntimePhase::Stopping {
                            requested_at_millis: 1000,
                        },
                    )
                    .expect("Stop intent");
            }
            _ => {
                saved = fixture.observe();
                let mut state = saved.workspace().state().clone();
                let runtime = state.runtime.as_mut().expect("runtime");
                if mode == 6 {
                    runtime.phase = RemoteRuntimePhase::Reconciling;
                } else {
                    runtime.phase = RemoteRuntimePhase::Deleting;
                    runtime.cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::WorkspaceRemoved,
                        requested_at_millis: 1000,
                    });
                }
                fixture
                    .store
                    .replace_remote_workspace(saved.workspace(), &state)
                    .expect("phase");
                saved = fixture.reload();
            }
        }
        let before = fixture.snapshots();
        assert!(matches!(
            fixture
                .store
                .record_remote_network_volume_selection(&saved, &selection()),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert_eq!(fixture.count(), 0);
        assert_eq!(fixture.snapshots(), before);
    }
}

#[test]
fn concurrent_different_selections_have_one_immutable_winner() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let store = fixture.store.clone();
            let saved = fixture.saved.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let chosen = RunPodNetworkVolumeExpectation {
                    volume_id: format!("volume_{index}"),
                    ..selection()
                };
                barrier.wait();
                store.record_remote_network_volume_selection(&saved, &chosen)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::ReplacementIdentityMismatch)))
            .count(),
        1
    );
    assert_eq!(fixture.count(), 1);
}

#[test]
fn selected_management_state_is_readable_without_revision_or_intent_changes() {
    let fixture = Fixture::new();
    fixture
        .store
        .record_remote_network_volume_selection(&fixture.saved, &selection())
        .expect("record");
    let stopping = fixture
        .store
        .record_remote_stop_phase(
            &fixture.observe(),
            RemoteRuntimePhase::Stopping {
                requested_at_millis: 1000,
            },
        )
        .expect("Stop intent");
    let before = fixture.snapshots();
    fixture
        .store
        .record_remote_network_volume_selection(&stopping, &selection())
        .expect("same selection");
    assert_eq!(
        fixture
            .store
            .load_remote_network_volume_selection(&stopping)
            .expect("read"),
        Some(selection())
    );
    assert_eq!(fixture.snapshots(), before);
    assert_eq!(fixture.reload(), stopping);
    assert_eq!(fixture.count(), 1);
}
