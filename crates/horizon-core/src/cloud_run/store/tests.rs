use std::sync::{Arc, Barrier};

use rusqlite::Connection;
use tempfile::TempDir;

use super::*;
use crate::cloud_run::{
    CLOUD_RUN_PROTOCOL_VERSION, CloudJobOutcome, CloudJobState, CloudProgress, RetryPolicy, WorkerTarget, WorkflowNode,
    WorkflowNodeKind,
};

fn test_workflow(provider: CloudProvider, timestamp: i64) -> CloudWorkflow {
    CloudWorkflow {
        protocol_version: CLOUD_RUN_PROTOCOL_VERSION,
        id: CloudWorkflowId::new(),
        title: "remote build".to_string(),
        created_at_millis: timestamp,
        updated_at_millis: timestamp,
        retain_until_millis: timestamp + 60_000,
        nodes: vec![WorkflowNode {
            id: CloudJobId::new(),
            logical_key: "build".to_string(),
            label: "Build".to_string(),
            kind: WorkflowNodeKind::Build,
            state: CloudJobState::Queued,
            outcome: None,
            progress: CloudProgress::Pending,
            weight: 1,
            attempt: 1,
            retry: RetryPolicy::default(),
            supersedes: None,
            depends_on: Vec::new(),
            source: None,
            worker: Some(WorkerTarget {
                provider,
                profile: "general".to_string(),
                image: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
                disk_gib: 20,
                lease_seconds: 3_600,
                max_hourly_cost_micros: Some(500_000),
            }),
            input_artifact_ids: Vec::new(),
            outputs: Vec::new(),
            approval: None,
            release: None,
            environment_lease: None,
        }],
    }
}

fn retained_workflow(provider: CloudProvider, timestamp: i64) -> CloudWorkflow {
    let mut workflow = test_workflow(provider, timestamp);
    workflow.retain_until_millis = i64::MAX;
    workflow
}

fn store(temp: &TempDir) -> CloudWorkflowStore {
    CloudWorkflowStore::open_path(temp.path().join("cloud-run").join("workflows.sqlite3")).expect("store")
}

#[test]
fn snapshots_round_trip_and_revision_compare_and_swap_is_exact() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let original = test_workflow(CloudProvider::RunPod, 1_000);
    let stored = store.create(&original).expect("create workflow");
    assert_eq!(stored.revision(), 1);
    assert_eq!(stored.workflow(), &original);
    assert_eq!(store.load(original.id).expect("load workflow"), Some(stored.clone()));
    assert!(matches!(
        store.create(&original),
        Err(CloudStoreError::WorkflowExists(id)) if id == original.id
    ));

    let mut next = original.clone();
    next.updated_at_millis += 1;
    next.nodes[0].state = CloudJobState::Provisioning;
    let replaced = store.replace(&stored, &next).expect("replace workflow");
    assert_eq!(replaced.revision(), 2);
    assert_eq!(replaced.workflow(), &next);

    let mut stale = next.clone();
    stale.updated_at_millis += 1;
    assert!(matches!(
        store.replace(&stored, &stale),
        Err(CloudStoreError::RevisionConflict { expected: 1, actual: 2 })
    ));
    assert_eq!(store.load(original.id).expect("load current"), Some(replaced));
}

#[test]
fn replacement_preserves_identity_time_and_retention() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let original = test_workflow(CloudProvider::Azure, 2_000);
    let stored = store.create(&original).expect("create workflow");

    let mut changed_id = original.clone();
    changed_id.id = CloudWorkflowId::new();
    changed_id.updated_at_millis += 1;
    assert!(matches!(
        store.replace(&stored, &changed_id),
        Err(CloudStoreError::ReplacementIdentityMismatch)
    ));
    let mut stale_time = original.clone();
    stale_time.retain_until_millis -= 1;
    assert!(matches!(
        store.replace(&stored, &stale_time),
        Err(CloudStoreError::NonMonotonicReplacement)
    ));
    let mut shorter_retention = original.clone();
    shorter_retention.updated_at_millis += 1;
    shorter_retention.retain_until_millis -= 1;
    assert!(matches!(
        store.replace(&stored, &shorter_retention),
        Err(CloudStoreError::NonMonotonicReplacement)
    ));
    assert_eq!(store.load(original.id).expect("load unchanged"), Some(stored));
}

#[test]
fn recovery_lists_only_retained_valid_snapshots() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let expired = test_workflow(CloudProvider::RunPod, 1_000);
    let retained = test_workflow(CloudProvider::Azure, 100_000);
    store.create(&expired).expect("create expired");
    store.create(&retained).expect("create retained");

    let recovered = store.list_retained(70_000).expect("list retained");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].workflow(), &retained);

    let connection = Connection::open(store.path()).expect("open raw store");
    connection
        .execute(
            "UPDATE cloud_workflows SET snapshot = X'7B7D' WHERE workflow_id = ?1",
            [retained.id.to_string()],
        )
        .expect("corrupt snapshot");
    assert!(matches!(
        store.load(retained.id),
        Err(CloudStoreError::InvalidStoredWorkflow { workflow_id, .. }) if workflow_id == retained.id
    ));
}

#[test]
fn recovery_budget_rejects_unbounded_snapshot_sets() {
    assert!(matches!(
        check_recovery_budget(MAX_RECOVERED_WORKFLOWS + 1, 0),
        Err(CloudStoreError::RecoveryLimitExceeded)
    ));
    assert!(matches!(
        check_recovery_budget(1, MAX_RECOVERED_SNAPSHOT_BYTES + 1),
        Err(CloudStoreError::RecoveryLimitExceeded)
    ));
}

#[test]
fn creation_claim_is_durable_atomic_and_bound_to_the_persisted_job() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let workflow = retained_workflow(CloudProvider::RunPod, 1_000);
    let job_id = workflow.nodes[0].id;
    store.create(&workflow).expect("create workflow");
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let store = store.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            store.claim_worker_creation(CloudProvider::RunPod, workflow.id, job_id, "horizon-worker-1")
        }));
    }
    barrier.wait();
    let mut outcomes = threads
        .into_iter()
        .map(|thread| thread.join().expect("claim thread").expect("claim"))
        .collect::<Vec<_>>();
    outcomes.sort_unstable();
    assert_eq!(outcomes, vec![false, true]);

    let reopened = CloudWorkflowStore::open_path(store.path().to_path_buf()).expect("reopen store");
    assert!(
        !reopened
            .claim_worker_creation(CloudProvider::RunPod, workflow.id, job_id, "horizon-worker-1")
            .expect("repeat claim")
    );
    assert!(matches!(
        reopened.claim_worker_creation(CloudProvider::Azure, workflow.id, job_id, "horizon-worker-1"),
        Err(CloudStoreError::ClaimTargetMismatch(id)) if id == job_id
    ));
    assert!(matches!(
        reopened.claim_worker_creation(CloudProvider::RunPod, workflow.id, job_id, "bad/name"),
        Err(CloudStoreError::InvalidResourceName)
    ));

    let other = retained_workflow(CloudProvider::RunPod, 3_000);
    let other_job = other.nodes[0].id;
    reopened.create(&other).expect("create other workflow");
    assert!(matches!(
        reopened.claim_worker_creation(CloudProvider::RunPod, other.id, other_job, "horizon-worker-1"),
        Err(CloudStoreError::ClaimIdentityConflict)
    ));
    assert!(matches!(
        reopened.claim_worker_creation(CloudProvider::RunPod, workflow.id, job_id, "horizon-worker-2"),
        Err(CloudStoreError::ClaimIdentityConflict)
    ));
}

#[test]
fn expired_terminal_and_dependency_blocked_jobs_cannot_claim_creation() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);

    let expired = test_workflow(CloudProvider::RunPod, 1_000);
    let expired_job = expired.nodes[0].id;
    store.create(&expired).expect("create expired workflow");
    assert!(matches!(
        store.claim_worker_creation(CloudProvider::RunPod, expired.id, expired_job, "horizon-expired"),
        Err(CloudStoreError::WorkflowExpired(id)) if id == expired.id
    ));

    let mut terminal = retained_workflow(CloudProvider::RunPod, 10_000);
    let terminal_job = terminal.nodes[0].id;
    terminal.nodes[0].state = CloudJobState::Completed;
    terminal.nodes[0].outcome = Some(CloudJobOutcome::Succeeded);
    terminal.nodes[0].progress = CloudProgress::Completed;
    store.create(&terminal).expect("create terminal workflow");
    assert!(matches!(
        store.claim_worker_creation(CloudProvider::RunPod, terminal.id, terminal_job, "horizon-terminal"),
        Err(CloudStoreError::ClaimTargetNotReady(id)) if id == terminal_job
    ));

    let mut blocked = retained_workflow(CloudProvider::RunPod, 20_000);
    let blocked_job = blocked.nodes[0].id;
    let mut dependency = blocked.nodes[0].clone();
    dependency.id = CloudJobId::new();
    dependency.logical_key = "prepare".to_string();
    dependency.label = "Prepare".to_string();
    dependency.worker = None;
    let dependency_id = dependency.id;
    blocked.nodes[0].depends_on.push(dependency_id);
    blocked.nodes.push(dependency);
    let stored = store.create(&blocked).expect("create dependency-blocked workflow");
    assert!(matches!(
        store.claim_worker_creation(CloudProvider::RunPod, blocked.id, blocked_job, "horizon-blocked"),
        Err(CloudStoreError::ClaimTargetNotReady(id)) if id == blocked_job
    ));

    let mut ready = blocked;
    ready.updated_at_millis += 1;
    ready.nodes[1].state = CloudJobState::Completed;
    ready.nodes[1].outcome = Some(CloudJobOutcome::Succeeded);
    ready.nodes[1].progress = CloudProgress::Completed;
    store.replace(&stored, &ready).expect("complete dependency");
    assert!(
        store
            .claim_worker_creation(CloudProvider::RunPod, ready.id, blocked_job, "horizon-ready")
            .expect("claim ready worker")
    );
}

#[test]
fn runpod_fence_uses_the_durable_workflow_claim() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let workflow = retained_workflow(CloudProvider::RunPod, 30_000);
    let job_id = workflow.nodes[0].id;
    store.create(&workflow).expect("create workflow");

    assert!(
        super::super::runpod::RunPodCreationFence::claim_once(&store, workflow.id, job_id, "horizon-worker-trait")
            .expect("first claim")
    );
    assert!(
        !super::super::runpod::RunPodCreationFence::claim_once(&store, workflow.id, job_id, "horizon-worker-trait")
            .expect("repeated claim")
    );
}

#[test]
fn invalid_snapshots_and_future_schema_fail_closed() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let mut invalid = test_workflow(CloudProvider::Azure, 1_000);
    invalid.title.clear();
    assert!(matches!(store.create(&invalid), Err(CloudStoreError::Protocol(_))));
    assert!(store.load(invalid.id).expect("load missing").is_none());

    let future_path = temp.path().join("future").join("workflows.sqlite3");
    CloudWorkflowStore::open_path(&future_path).expect("initialize future store");
    let connection = Connection::open(&future_path).expect("future store");
    connection
        .pragma_update(None, "user_version", 2)
        .expect("future version");
    drop(connection);
    assert!(matches!(
        CloudWorkflowStore::open_path(future_path),
        Err(CloudStoreError::UnsupportedSchema(2))
    ));
}

#[cfg(unix)]
#[test]
fn store_directory_and_database_are_private() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let directory_mode = store
        .path()
        .parent()
        .expect("store parent")
        .metadata()
        .expect("metadata")
        .permissions()
        .mode();
    let database_mode = store.path().metadata().expect("metadata").permissions().mode();
    assert_eq!(directory_mode & 0o777, 0o700);
    assert_eq!(database_mode & 0o777, 0o600);

    let shared = temp.path().join("shared");
    std::fs::create_dir(&shared).expect("shared dir");
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).expect("shared permissions");
    assert!(matches!(
        CloudWorkflowStore::open_path(shared.join("workflows.sqlite3")),
        Err(CloudStoreError::InsecureStoreDirectory)
    ));
    assert_eq!(
        shared.metadata().expect("shared metadata").permissions().mode() & 0o777,
        0o755
    );

    let linked = store.path().parent().expect("store parent").join("linked.sqlite3");
    symlink(store.path(), &linked).expect("store symlink");
    assert!(matches!(
        CloudWorkflowStore::open_path(linked),
        Err(CloudStoreError::SymlinkStorePath)
    ));
}

#[test]
fn completed_snapshot_remains_protocol_valid_after_replacement() {
    let temp = TempDir::new().expect("temp dir");
    let store = store(&temp);
    let original = test_workflow(CloudProvider::RunPod, 1_000);
    let stored = store.create(&original).expect("create workflow");
    let mut completed = original;
    completed.updated_at_millis += 1;
    completed.nodes[0].state = CloudJobState::Completed;
    completed.nodes[0].outcome = Some(CloudJobOutcome::Succeeded);
    completed.nodes[0].progress = CloudProgress::Completed;
    let replaced = store.replace(&stored, &completed).expect("complete workflow");
    assert_eq!(replaced.workflow(), &completed);
}
