use super::super::super::super::database::ensure_current_schema;
use super::super::super::{StoredRemoteWorkspace, WorkspaceReplacement};
use super::{OWNER, assert_denied, fence_count, retained_workflow};
use crate::cloud_run::{
    CloudJobId, CloudProvider, CloudWorkflow, CloudWorkflowId, CloudWorkflowStore, GitCommitSha, GitSource,
    WorkerLifetime,
};
use crate::remote_workspace::{RemoteRuntimeGeneration, RemoteRuntimePhase, RemoteWorkspaceSpec, RemoteWorkspaceState};
use rusqlite::{Connection, Transaction, TransactionBehavior};

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    workflow: CloudWorkflow,
    dormant: StoredRemoteWorkspace,
    active: RemoteWorkspaceState,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("private directory");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let mut workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
    workflow.nodes[0].source = Some(GitSource {
        repository: "owner/project".into(),
        commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
        branch: None,
    });
    let target = workflow.nodes[0].worker.as_mut().expect("target");
    target.lifetime = WorkerLifetime::Persistent;
    let state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: "workspace".into(),
        target: target.clone(),
        repository: workflow.nodes[0].source.clone().expect("source"),
        working_directory: ".".into(),
        generation: 0,
        panels: Vec::new(),
    })
    .expect("dormant specification");
    store.create(&workflow).expect("ordinary workflow");
    let dormant = store.create_remote_workspace(OWNER, &state).expect("dormant record");
    let mut active = state;
    active.spec.generation = 1;
    active.runtime = Some(RemoteRuntimeGeneration {
        workspace_local_id: "workspace".into(),
        generation: 1,
        workflow_id: workflow.id,
        job_id: workflow.nodes[0].id,
        phase: RemoteRuntimePhase::Provisioning,
        worker: None,
        ssh: None,
        cleanup: None,
    });
    Fixture {
        _directory: directory,
        store,
        workflow,
        dormant,
        active,
    }
}

impl Fixture {
    fn stage(&self, transaction: &Transaction<'_>) -> StoredRemoteWorkspace {
        ensure_current_schema(transaction).expect("current schema");
        WorkspaceReplacement::new(&self.dormant, &self.active)
            .expect("prepare runtime snapshot")
            .persist(transaction)
            .expect("stage runtime snapshot")
    }
}

#[test]
fn runtime_snapshot_and_creation_denial_commit_or_rollback_together() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut connection = Connection::open(store.path()).expect("raw store");
    assert_eq!(fence_count(&connection), 0);
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("write transaction");
    let pending = fixture.stage(&transaction);
    assert_eq!(pending.state(), &fixture.active);
    assert_eq!(fence_count(&transaction), 1);
    transaction.rollback().expect("roll back staged runtime");
    assert_eq!(fence_count(&connection), 0);
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(fixture.dormant.clone())
    );

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("commit transaction");
    let saved = fixture.stage(&transaction);
    transaction.commit().expect("commit runtime with denial");
    assert_eq!(saved, pending);
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_workspace(OWNER, "workspace").expect("retained"),
        Some(saved.clone())
    );
    assert_denied(&reopened, &fixture.workflow);
    let mut next = saved.state().clone();
    next.spec.working_directory = "nested".into();
    reopened
        .replace_remote_workspace(&saved, &next)
        .expect("repeat runtime write");
    assert_eq!(fence_count(&connection), 1);
}

#[test]
fn newly_persisted_runtime_ids_cannot_be_reused_after_retirement() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut connection = Connection::open(store.path()).expect("raw store");
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("runtime transaction");
    let saved = fixture.stage(&transaction);
    transaction.commit().expect("commit runtime");

    let mut copied = fixture.workflow.clone();
    copied.id = CloudWorkflowId::new();
    store.create(&copied).expect("ordinary workflow with copied job");
    assert_denied(store, &copied);
    let original = store
        .load(fixture.workflow.id)
        .expect("load")
        .expect("original workflow");
    let mut changed_job = original.workflow().clone();
    changed_job.nodes[0].id = CloudJobId::new();
    changed_job.updated_at_millis += 1;
    store
        .replace(&original, &changed_job)
        .expect("ordinary workflow with new job");
    assert_denied(store, &changed_job);

    let mut retired = saved.state().clone();
    retired.runtime = None;
    store
        .replace_remote_workspace(&saved, &retired)
        .expect("retire unbound runtime");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen retired runtime");
    assert_denied(&reopened, &copied);
    assert_denied(&reopened, &changed_job);
    assert_eq!(fence_count(&connection), 1);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM cloud_worker_creation_claims", [], |row| row
                .get::<_, i64>(0))
            .expect("no claims"),
        0
    );
    let ordinary = retained_workflow(CloudProvider::LocalDocker, 1000);
    reopened.create(&ordinary).expect("unrelated ordinary workflow");
    assert!(
        reopened
            .claim_worker_creation(
                ordinary.id,
                ordinary.nodes[0].id,
                ordinary.nodes[0].worker.as_ref().expect("target"),
                "unrelated-worker"
            )
            .expect("unrelated grant")
    );
}
