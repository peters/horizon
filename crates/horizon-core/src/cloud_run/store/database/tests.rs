use super::super::{CloudWorkflowStore, StoredRemoteWorkspace, StoredWorkflow, tests::retained_workflow};
use super::*;
use crate::cloud_run::{CloudProvider, GitCommitSha, GitSource};
use crate::remote_workspace::{RemoteWorkspaceSpec, RemoteWorkspaceState};
use rusqlite::params;

const OWNER: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    workflow: StoredWorkflow,
    workspace: StoredRemoteWorkspace,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("private directory");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
    let target = workflow.nodes[0].worker.clone().expect("target");
    let saved_workflow = store.create(&workflow).expect("legacy workflow");
    assert!(
        store
            .claim_worker_creation(workflow.id, workflow.nodes[0].id, &target, "synthetic-worker")
            .expect("claim")
    );
    let state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: "workspace".into(),
        target,
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: ".".into(),
        generation: 0,
        panels: Vec::new(),
    })
    .expect("remote specification");
    let workspace = store.create_remote_workspace(OWNER, &state).expect("remote record");
    Fixture {
        _directory: directory,
        store,
        workflow: saved_workflow,
        workspace,
    }
}

#[derive(Debug, PartialEq)]
struct SavedRows {
    workflow: Vec<u8>,
    workspace: Vec<u8>,
    claims: Vec<SavedClaim>,
}

#[derive(Debug, PartialEq)]
struct SavedClaim {
    provider: String,
    workflow_id: String,
    job_id: String,
    resource_name: String,
    claimed_at_millis: i64,
}

fn saved_bytes(connection: &Connection) -> SavedRows {
    let (workflow, workspace) = connection
        .query_row(
            "SELECT (SELECT snapshot FROM cloud_workflows), (SELECT snapshot FROM remote_workspaces)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("preserved snapshots");
    let mut statement = connection
        .prepare(
            "SELECT provider, workflow_id, job_id, resource_name, claimed_at_millis
             FROM cloud_worker_creation_claims ORDER BY provider, resource_name",
        )
        .expect("claims query");
    let claims = statement
        .query_map([], |row| {
            Ok(SavedClaim {
                provider: row.get(0)?,
                workflow_id: row.get(1)?,
                job_id: row.get(2)?,
                resource_name: row.get(3)?,
                claimed_at_millis: row.get(4)?,
            })
        })
        .expect("all claims")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("saved claims");
    SavedRows {
        workflow,
        workspace,
        claims,
    }
}

fn allocation_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM remote_runtime_allocations", [], |row| row.get(0))
        .expect("binding count")
}

#[test]
fn schema_two_upgrade_preserves_records_and_claims_without_inventing_allocations() {
    let fixture = fixture();
    let connection = open_connection(fixture.store.path()).expect("raw store");
    connection
        .execute_batch(
            "DROP TABLE remote_runtime_creation_fences; DROP TABLE remote_runtime_allocations; PRAGMA user_version=2;",
        )
        .expect("schema two fixture");
    let before = saved_bytes(&connection);
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("version"),
        4
    );
    assert_eq!(allocation_count(&connection), 0);
    let workflow = fixture.workflow.workflow();
    assert_eq!(
        upgraded.load(workflow.id).expect("workflow"),
        Some(fixture.workflow.clone())
    );
    assert_eq!(
        upgraded.load_remote_workspace(OWNER, "workspace").expect("workspace"),
        Some(fixture.workspace)
    );
    assert!(
        !upgraded
            .claim_worker_creation(
                workflow.id,
                workflow.nodes[0].id,
                workflow.nodes[0].worker.as_ref().expect("target"),
                "synthetic-worker"
            )
            .expect("claim retained")
    );
    CloudWorkflowStore::open_path(fixture.store.path()).expect("idempotent reopen");
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(allocation_count(&connection), 0);
}

#[test]
fn partial_or_missing_allocation_schema_fails_without_repairing_or_losing_records() {
    for sql in [
        "DROP TABLE remote_runtime_allocations",
        "DROP INDEX remote_runtime_allocations_workflow",
        "DROP INDEX remote_runtime_allocations_job",
        "PRAGMA user_version=2",
    ] {
        let fixture = fixture();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        let before = saved_bytes(&connection);
        connection.execute_batch(sql).expect("partial schema");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert!(CloudWorkflowStore::open_path(fixture.store.path()).is_err(), "{sql}");
        assert_eq!(saved_bytes(&connection), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged version"),
            version
        );
    }
}

#[test]
fn schema_three_definitions_match_the_frozen_storage_format() {
    let frozen = concat!(
        "CREATE TABLE remote_runtime_allocations (\n",
        "    workspace_local_id TEXT PRIMARY KEY NOT NULL REFERENCES remote_workspaces(workspace_local_id),\n",
        "    session_id TEXT NOT NULL,\n",
        "    generation INTEGER NOT NULL CHECK (generation > 0),\n",
        "    workflow_id TEXT NOT NULL REFERENCES cloud_workflows(workflow_id),\n",
        "    job_id TEXT NOT NULL\n",
        ") STRICT;\n",
        "CREATE UNIQUE INDEX remote_runtime_allocations_workflow ON remote_runtime_allocations(workflow_id);\n",
        "CREATE UNIQUE INDEX remote_runtime_allocations_job ON remote_runtime_allocations(job_id)",
    );
    assert_eq!(REMOTE_ALLOCATION_SCHEMA.join(";\n"), frozen);
}

#[test]
fn unexpected_allocation_objects_reject_open_reads_and_writes_without_record_loss() {
    for sql in [
        "CREATE INDEX unexpected_index ON remote_runtime_allocations(session_id)",
        "CREATE TRIGGER unexpected_trigger AFTER INSERT ON remote_runtime_allocations BEGIN
         DELETE FROM cloud_workflows; END;",
    ] {
        let fixture = fixture();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        let before = saved_bytes(&connection);
        connection.execute_batch(sql).expect("unexpected object");
        assert!(matches!(
            CloudWorkflowStore::open_path(fixture.store.path()),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        assert!(matches!(
            fixture.store.load(fixture.workflow.workflow().id),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        let workflow = retained_workflow(CloudProvider::LocalDocker, 2000);
        assert!(matches!(
            fixture.store.create(&workflow),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        assert_eq!(saved_bytes(&connection), before);
    }
}

#[test]
fn malformed_allocation_constraints_are_rejected_without_losing_records() {
    for (original, replacement) in [
        ("TEXT PRIMARY KEY NOT NULL", "TEXT NOT NULL"),
        ("session_id TEXT NOT NULL", "session_id TEXT"),
        ("CHECK (generation > 0)", "CHECK (generation >= 0)"),
        (" REFERENCES remote_workspaces(workspace_local_id)", ""),
        (" REFERENCES cloud_workflows(workflow_id)", ""),
        (
            "REFERENCES cloud_workflows(workflow_id)",
            "REFERENCES cloud_workflows(workflow_id) ON DELETE CASCADE",
        ),
        (
            "remote_runtime_allocations_workflow ON remote_runtime_allocations(workflow_id)",
            "remote_runtime_allocations_workflow ON remote_runtime_allocations(session_id)",
        ),
        ("CREATE UNIQUE INDEX", "CREATE INDEX"),
        (
            "remote_runtime_allocations(job_id)",
            "remote_runtime_allocations(job_id) WHERE generation = 1",
        ),
        (
            "remote_runtime_allocations(job_id)",
            "remote_runtime_allocations(job_id, generation)",
        ),
        (
            "remote_runtime_allocations(workflow_id)",
            "remote_runtime_allocations(lower(workflow_id))",
        ),
        (
            "remote_runtime_allocations(job_id)",
            "remote_runtime_allocations(job_id COLLATE NOCASE)",
        ),
        (
            "remote_runtime_allocations(job_id)",
            "remote_runtime_allocations(job_id DESC)",
        ),
        (
            "remote_runtime_allocations(workflow_id)",
            "remote_runtime_allocations(workflow_id) WHERE generation = 1",
        ),
        (
            "remote_runtime_allocations(job_id)",
            "remote_runtime_allocations(session_id)",
        ),
        (
            "CREATE UNIQUE INDEX remote_runtime_allocations_job",
            "CREATE INDEX remote_runtime_allocations_job",
        ),
        (
            "CREATE UNIQUE INDEX remote_runtime_allocations_workflow",
            "CREATE INDEX remote_runtime_allocations_workflow",
        ),
        (
            "REFERENCES remote_workspaces(workspace_local_id)",
            "REFERENCES remote_workspaces(workspace_local_id) ON DELETE CASCADE",
        ),
        (" STRICT", ""),
    ] {
        let fixture = fixture();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        let before = saved_bytes(&connection);
        let schema = REMOTE_ALLOCATION_SCHEMA.join(";\n");
        let malformed = schema.replace(original, replacement);
        assert_ne!(malformed, schema);
        connection
            .execute_batch("DROP TABLE remote_runtime_allocations")
            .expect("remove empty table");
        connection.execute_batch(&malformed).expect("malformed fixture");
        assert!(
            CloudWorkflowStore::open_path(fixture.store.path()).is_err(),
            "{original} -> {replacement}"
        );
        assert!(matches!(
            fixture.store.load(fixture.workflow.workflow().id),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        assert_eq!(saved_bytes(&connection), before);
    }
}

#[test]
fn allocation_schema_restricts_parent_loss_and_duplicate_workflow_or_job_identity() {
    let fixture = fixture();
    let connection = open_connection(fixture.store.path()).expect("foreign-key enabled connection");
    let workflow = fixture.workflow.workflow();
    let mut other_state = fixture.workspace.state().clone();
    other_state.spec.workspace_local_id = "second".into();
    fixture
        .store
        .create_remote_workspace(OWNER, &other_state)
        .expect("second workspace");
    let other = retained_workflow(CloudProvider::LocalDocker, 2000);
    fixture.store.create(&other).expect("second workflow");
    let before = saved_bytes(&connection);
    let insert = "INSERT INTO remote_runtime_allocations
        (workspace_local_id, session_id, generation, workflow_id, job_id) VALUES (?1, ?2, ?3, ?4, ?5)";
    let first_workflow = workflow.id.to_string();
    let first_job = workflow.nodes[0].id.to_string();
    let other_workflow = other.id.to_string();
    let other_job = other.nodes[0].id.to_string();
    connection
        .execute(insert, params!["workspace", OWNER, 1, first_workflow, first_job])
        .expect("synthetic schema binding");
    for (workspace_id, generation, workflow_id, job_id) in [
        ("second", 1, &first_workflow, &other_job),
        ("second", 1, &other_workflow, &first_job),
        ("workspace", 1, &other_workflow, &other_job),
        ("missing", 1, &other_workflow, &other_job),
        ("second", 0, &other_workflow, &other_job),
        ("second", 1, &other_job, &other_job),
    ] {
        assert!(
            connection
                .execute(insert, params![workspace_id, OWNER, generation, workflow_id, job_id])
                .is_err()
        );
        assert_eq!(allocation_count(&connection), 1);
    }
    assert!(
        connection
            .execute("DELETE FROM cloud_workflows WHERE workflow_id=?1", [&first_workflow])
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM remote_workspaces WHERE workspace_local_id='workspace'", [])
            .is_err()
    );
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(
        fixture.store.load(workflow.id).expect("parent retained"),
        Some(fixture.workflow.clone())
    );
    assert_eq!(
        fixture
            .store
            .load_remote_workspace(OWNER, "workspace")
            .expect("parent retained"),
        Some(fixture.workspace.clone())
    );
    connection
        .execute(insert, params!["second", OWNER, 1, other_workflow, other_job])
        .expect("independent identity");
    assert_eq!(allocation_count(&connection), 2);
}
