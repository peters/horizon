use super::*;
use crate::cloud_run::{CloudJobState, CloudProgress, CloudWorkflow, RetryPolicy, WorkflowNode, WorkflowNodeKind};

pub(super) struct CreationStore {
    pub(super) store: CloudWorkflowStore,
    _directory: tempfile::TempDir,
}

impl CreationStore {
    pub(super) fn new(request: &InteractiveWorkerRequest) -> Self {
        let directory = tempfile::tempdir().expect("isolated creation fence");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        store
            .create(&CloudWorkflow {
                protocol_version: CLOUD_RUN_PROTOCOL_VERSION,
                id: request.workflow_id,
                title: "Local worker fixture".into(),
                created_at_millis: 1000,
                updated_at_millis: 1000,
                retain_until_millis: i64::MAX,
                nodes: vec![WorkflowNode {
                    id: request.job_id,
                    logical_key: "worker".into(),
                    label: "Worker".into(),
                    kind: WorkflowNodeKind::Build,
                    state: CloudJobState::Queued,
                    outcome: None,
                    progress: CloudProgress::Pending,
                    weight: 1,
                    attempt: 1,
                    retry: RetryPolicy::default(),
                    supersedes: None,
                    depends_on: vec![],
                    source: None,
                    worker: Some(request.target.clone()),
                    input_artifact_ids: vec![],
                    outputs: vec![],
                    approval: None,
                    release: None,
                    environment_lease: None,
                }],
            })
            .expect("workflow");
        Self {
            store,
            _directory: directory,
        }
    }
}

fn claims(store: &CloudWorkflowStore) -> i64 {
    rusqlite::Connection::open(store.path())
        .expect("database")
        .query_row("SELECT COUNT(*) FROM cloud_worker_creation_claims", [], |row| {
            row.get(0)
        })
        .expect("claims")
}

#[test]
fn creation_claim_survives_controller_drop_and_resource_absence() {
    let request = lifetime::persistent_request();
    let fake = FakeDocker::default();
    let first = provider_for("local", fake.clone(), &request);
    let worker = first
        .ensure_worker(&request)
        .expect("first creation")
        .into_status()
        .worker;
    assert_eq!(claims(&first.creation_store), 1);
    let path = first.creation_store.path().to_path_buf();
    drop(first);
    fake.state().container = None;
    let reopened = LocalDockerInteractiveWorkerProvider {
        transport: Box::new(fake.clone()),
        profile: profile("local", "unix:///var/run/docker.sock"),
        creation_store: CloudWorkflowStore::open_path(path).expect("reopen fence"),
    };
    assert_eq!(reopened.inspect_worker(&worker), Ok(None));
    assert_eq!(reopened.reconcile_worker(&request), Ok(None));
    for _ in 0..3 {
        assert_eq!(reopened.ensure_worker(&request), Err(CreationReconciliationRequired));
    }
    assert_eq!(claims(&reopened.creation_store), 1);
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 0));
}

#[test]
fn a_failed_first_create_keeps_its_grant_and_a_retry_cannot_create() {
    let request = lifetime::persistent_request();
    let fake = FakeDocker::default();
    fake.state().reject_create = true;
    let provider = provider_for("local", fake.clone(), &request);
    assert!(provider.ensure_worker(&request).is_err());
    fake.state().reject_create = false;
    assert_eq!(provider.ensure_worker(&request), Err(CreationReconciliationRequired));
    assert_eq!(claims(&provider.creation_store), 1);
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 0));
}

#[test]
fn simultaneous_controllers_share_one_durable_creation_grant() {
    let request = lifetime::persistent_request();
    let fake = FakeDocker::default();
    let providers = [
        provider_for("local", fake.clone(), &request),
        provider_for("local", fake.clone(), &request),
    ];
    let store = providers[0].creation_store.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = providers
        .into_iter()
        .map(|provider| {
            let barrier = Arc::clone(&barrier);
            let request = request.clone();
            std::thread::spawn(move || {
                barrier.wait();
                provider.ensure_worker(&request)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("controller"))
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(InteractiveWorkerEnsure::Created(_))))
            .count(),
        1
    );
    assert!(
        results
            .iter()
            .all(|result| matches!(result, Ok(_) | Err(CreationReconciliationRequired)))
    );
    assert_eq!(claims(&store), 1);
    let state = fake.state();
    assert_eq!((state.create_calls, state.delete_calls), (1, 0));
}

#[test]
fn observed_disappearance_does_not_fall_through_to_creation_even_with_an_unused_grant() {
    let request = lifetime::persistent_request();
    let fake = noncreating::seeded(&request);
    fake.state().disappear_during_key_read = true;
    let provider = provider_for("local", fake.clone(), &request);
    assert_eq!(provider.ensure_worker(&request), Err(ResourceAbsent));
    assert_eq!(claims(&provider.creation_store), 0);
    noncreating::assert_read_only(&fake);
}

#[test]
fn expired_or_corrupt_fences_fail_before_any_creation_without_leaking_storage_detail() {
    for corrupt in [false, true] {
        let fake = FakeDocker::default();
        let request = request();
        let provider = provider("local", fake.clone());
        let connection = rusqlite::Connection::open(provider.creation_store.path()).expect("database");
        if corrupt {
            connection
                .execute(
                    "UPDATE cloud_workflows SET snapshot=?1",
                    [b"synthetic-private-snapshot".as_slice()],
                )
                .expect("corrupt");
        } else {
            let mut workflow = provider
                .creation_store
                .load(request.workflow_id)
                .expect("load")
                .expect("workflow")
                .workflow()
                .clone();
            workflow.retain_until_millis = 2000;
            connection
                .execute(
                    "UPDATE cloud_workflows SET retain_until_millis=2000, snapshot=?1",
                    [serde_json::to_vec(&workflow).expect("expired snapshot")],
                )
                .expect("expire");
        }
        let error = provider.ensure_worker(&request).expect_err("fence failed");
        assert_eq!(error, CreationFenceFailed);
        assert!(!format!("{error:?} {error}").contains("synthetic-private-snapshot"));
        assert_eq!(claims(&provider.creation_store), 0);
        noncreating::assert_read_only(&fake);
    }
}

#[test]
fn noncreating_inspection_and_reconciliation_do_not_consume_a_grant() {
    let request = request();
    let fake = FakeDocker::default();
    let provider = provider("local", fake.clone());
    assert_eq!(provider.reconcile_worker(&request), Ok(None));
    assert_eq!(claims(&provider.creation_store), 0);
    let seeded = noncreating::seeded(&request);
    let reader = provider_for("local", seeded.clone(), &request);
    let observed = reader.reconcile_worker(&request).expect("reconcile").expect("existing");
    assert_eq!(reader.inspect_worker(&observed.worker), Ok(Some(observed)));
    assert_eq!(claims(&reader.creation_store), 0);
    noncreating::assert_read_only(&seeded);
}

#[test]
fn missing_or_mismatched_workflow_targets_cannot_claim_creation() {
    for fault in 0..3 {
        let fake = FakeDocker::default();
        let provider = provider("local", fake.clone());
        let mut changed = request();
        match fault {
            0 => changed.workflow_id = crate::cloud_run::CloudWorkflowId::new(),
            1 => changed.job_id = crate::cloud_run::CloudJobId::new(),
            _ => changed.target.disk_gib += 1,
        }
        assert_eq!(provider.ensure_worker(&changed), Err(CreationFenceFailed));
        assert_eq!(claims(&provider.creation_store), 0);
        noncreating::assert_read_only(&fake);
    }
}

#[test]
fn owned_allocations_use_the_same_fence_and_management_intent_denies_new_creation() {
    use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState};
    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    for cancel in [false, true] {
        let directory = tempfile::tempdir().expect("owned fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1,
            "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": lifetime::persistent_request().target,
                "repository": { "repository": "example/project", "commit": "b".repeat(40) }
            }
        }))
        .expect("state");
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        let allocation = store
            .reserve_remote_worker_request(&allocation, &ed25519_key(3))
            .expect("request");
        let request = allocation.worker_request().expect("request");
        let fake = FakeDocker::default();
        let provider = LocalDockerInteractiveWorkerProvider {
            transport: Box::new(fake.clone()),
            profile: profile("local", "unix:///var/run/docker.sock"),
            creation_store: store.clone(),
        };
        if cancel {
            let mut state = allocation.workspace().state().clone();
            let runtime = state.runtime.as_mut().expect("runtime");
            runtime.phase = RemoteRuntimePhase::Cancelling;
            runtime.cleanup = Some(RemoteCleanupIntent {
                reason: RemoteCleanupReason::Cancelled,
                requested_at_millis: 1000,
            });
            store
                .replace_remote_workspace(allocation.workspace(), &state)
                .expect("management intent");
            assert_eq!(provider.ensure_worker(&request), Err(CreationFenceFailed));
            assert_eq!(claims(&store), 0);
            noncreating::assert_read_only(&fake);
        } else {
            assert!(matches!(
                provider.ensure_worker(&request),
                Ok(InteractiveWorkerEnsure::Created(_))
            ));
            assert_eq!(claims(&store), 1);
            fake.state().container = None;
            assert_eq!(provider.ensure_worker(&request), Err(CreationReconciliationRequired));
            assert_eq!(fake.state().create_calls, 1);
            assert_eq!(fake.state().delete_calls, 0);
        }
        assert_eq!(
            store
                .load_remote_allocation(OWNER, "workspace")
                .expect("reload")
                .expect("allocation")
                .workflow(),
            allocation.workflow()
        );
    }
}
