use super::super::super::tests::retained_workflow;
use super::*;
use crate::cloud_run::{CloudProvider, CloudWorkflow, CloudWorkflowStore, GitCommitSha, GitSource};
use crate::remote_workspace::{RemoteRuntimeGeneration, RemoteRuntimePhase, RemoteWorkspaceSpec, RemoteWorkspaceState};
use rusqlite::{Connection, params};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    workflow: CloudWorkflow,
    state: RemoteWorkspaceState,
}

fn fixture(version: i64) -> Fixture {
    let directory = tempfile::tempdir().expect("private directory");
    let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
    let workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
    store.create(&workflow).expect("ordinary unclaimed workflow");
    let mut state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: "legacy".into(),
        target: workflow.nodes[0].worker.clone().expect("target"),
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: ".".into(),
        generation: 1,
        panels: Vec::new(),
    })
    .expect("specification");
    state.runtime = Some(RemoteRuntimeGeneration {
        workspace_local_id: "legacy".into(),
        generation: 1,
        workflow_id: workflow.id,
        job_id: workflow.nodes[0].id,
        phase: RemoteRuntimePhase::Reconciling,
        worker: None,
        ssh: None,
        cleanup: None,
    });
    state.validate().expect("valid legacy runtime");
    let snapshot = serde_json::to_vec(&serde_json::json!({"session_id": OWNER, "state": state})).expect("snapshot");
    let connection = Connection::open(store.path()).expect("raw fixture");
    connection
        .execute(
            "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
            params!["legacy", OWNER, snapshot],
        )
        .expect("legacy record");
    connection
        .execute_batch("DROP TABLE remote_runtime_creation_fences;")
        .expect("legacy fence schema");
    if version == 2 {
        connection
            .execute_batch("DROP TABLE remote_runtime_allocations;")
            .expect("legacy allocation schema");
    }
    connection
        .pragma_update(None, "user_version", version)
        .expect("legacy version");
    Fixture {
        _directory: directory,
        store,
        workflow,
        state,
    }
}

#[test]
fn schema_two_unclaimed_active_runtime_never_gets_creation_authority() {
    let fixture = fixture(2);
    let connection = Connection::open(fixture.store.path()).expect("raw store");
    let before = snapshots(&connection);
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
    let saved = upgraded
        .load_remote_workspace(OWNER, "legacy")
        .expect("recover")
        .expect("saved runtime");
    assert_eq!(saved.state(), &fixture.state);
    assert_eq!(saved.revision(), 1);
    assert_denied(&upgraded, &fixture.workflow);
    let reopened = CloudWorkflowStore::open_path(upgraded.path()).expect("reopen");
    assert_denied(&reopened, &fixture.workflow);
    assert_eq!(snapshots(&connection), before);
    let counts: (i64, i64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM cloud_worker_creation_claims),
                    (SELECT COUNT(*) FROM remote_runtime_allocations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("claim and binding counts");
    assert_eq!(counts, (0, 0));
    assert_eq!(fence_count(&connection), 1);
}

fn snapshots(connection: &Connection) -> (Vec<u8>, Vec<u8>) {
    connection
        .query_row(
            "SELECT (SELECT snapshot FROM cloud_workflows), (SELECT snapshot FROM remote_workspaces)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("unchanged snapshots")
}

fn fence_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM remote_runtime_creation_fences", [], |row| {
            row.get(0)
        })
        .expect("fences")
}

fn assert_denied(store: &CloudWorkflowStore, workflow: &CloudWorkflow) {
    assert!(matches!(
        store.claim_worker_creation(
            workflow.id,
            workflow.nodes[0].id,
            workflow.nodes[0].worker.as_ref().expect("target"),
            "synthetic-worker"
        ),
        Err(CloudStoreError::LegacyRuntimeCreationDenied)
    ));
}

#[test]
fn schema_three_missing_workflows_and_reused_ids_remain_non_creating() {
    let fixture = fixture(3);
    let connection = Connection::open(fixture.store.path()).expect("raw store");
    connection
        .execute("DELETE FROM cloud_workflows", [])
        .expect("missing legacy workflow");
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade missing workflow");
    upgraded.create(&fixture.workflow).expect("recreated ordinary workflow");
    assert_denied(&upgraded, &fixture.workflow);
    let mut other = fixture.workflow.clone();
    other.id = CloudWorkflowId::new();
    upgraded.create(&other).expect("different workflow, referenced job");
    assert_denied(&upgraded, &other);
    let mut new_job = fixture.workflow.clone();
    new_job.nodes[0].id = CloudJobId::new();
    new_job.updated_at_millis += 1;
    let saved = upgraded.load(new_job.id).expect("load").expect("workflow");
    upgraded
        .replace(&saved, &new_job)
        .expect("referenced workflow, different job");
    assert_denied(&upgraded, &new_job);
    let ordinary = retained_workflow(CloudProvider::LocalDocker, 1000);
    upgraded.create(&ordinary).expect("unrelated ordinary workflow");
    assert!(
        upgraded
            .claim_worker_creation(
                ordinary.id,
                ordinary.nodes[0].id,
                ordinary.nodes[0].worker.as_ref().expect("target"),
                "unrelated-worker"
            )
            .expect("unrelated creation remains allowed")
    );
    assert_eq!(fence_count(&connection), 1);
}

#[test]
fn retiring_legacy_runtime_does_not_erase_its_creation_denial() {
    let fixture = fixture(3);
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
    let saved = upgraded
        .load_remote_workspace(OWNER, "legacy")
        .expect("load")
        .expect("runtime");
    let mut retired = saved.state().clone();
    retired.runtime = None;
    upgraded
        .replace_remote_workspace(&saved, &retired)
        .expect("retire legacy identity");
    assert_denied(&upgraded, &fixture.workflow);
    let reopened = CloudWorkflowStore::open_path(upgraded.path()).expect("reopen retired record");
    assert_eq!(
        reopened
            .load_remote_workspace(OWNER, "legacy")
            .expect("load")
            .expect("record")
            .state(),
        &retired
    );
    assert_denied(&reopened, &fixture.workflow);
    let connection = Connection::open(reopened.path()).expect("raw store");
    assert_eq!(fence_count(&connection), 1);
}

#[test]
fn migration_canonicalizes_uuid_denials_without_rewriting_legacy_bytes() {
    for compact in [false, true] {
        let fixture = fixture(3);
        let connection = Connection::open(fixture.store.path()).expect("raw store");
        let mut snapshot: serde_json::Value = serde_json::from_slice(&snapshots(&connection).1).expect("snapshot");
        for key in ["workflow_id", "job_id"] {
            let original = snapshot["state"]["runtime"][key]
                .as_str()
                .expect("UUID")
                .to_ascii_uppercase();
            snapshot["state"]["runtime"][key] =
                serde_json::json!(if compact { original.replace('-', "") } else { original });
        }
        connection
            .execute(
                "UPDATE remote_workspaces SET snapshot = ?1",
                [serde_json::to_vec(&snapshot).expect("variant bytes")],
            )
            .expect("legacy UUID spelling");
        let before = snapshots(&connection);
        let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade accepted UUID variant");
        assert_denied(&upgraded, &fixture.workflow);
        assert_eq!(snapshots(&connection), before);
        assert_eq!(fence_count(&connection), 1);
    }
}

#[test]
fn invalid_legacy_records_roll_back_schema_and_leave_snapshots_untouched() {
    for corrupt in [
        "UPDATE remote_workspaces SET snapshot = x'00'",
        "UPDATE remote_workspaces SET snapshot = zeroblob(4194305)",
        "UPDATE remote_workspaces SET session_id = 'invalid-owner'",
        "UPDATE remote_workspaces SET workspace_local_id = printf('%4096s', 'x')",
    ] {
        let fixture = fixture(3);
        let connection = Connection::open(fixture.store.path()).expect("raw store");
        connection.execute_batch(corrupt).expect("corrupt legacy fixture");
        let before = snapshots(&connection);
        assert!(matches!(
            CloudWorkflowStore::open_path(fixture.store.path()),
            Err(CloudStoreError::InvalidLegacyRuntimeSnapshot)
        ));
        assert_eq!(snapshots(&connection), before);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            3
        );
        let objects: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE tbl_name = 'remote_runtime_creation_fences'",
                [],
                |row| row.get(0),
            )
            .expect("rolled back fence objects");
        assert_eq!(objects, 0);
    }
}

#[test]
fn fence_schema_drift_blocks_open_and_existing_handle_operations() {
    for corrupt in [
        "DROP TABLE remote_runtime_creation_fences",
        "DROP INDEX remote_runtime_creation_fences_job",
        "DROP TRIGGER remote_runtime_creation_fences_no_update",
        "DROP TRIGGER remote_runtime_creation_fences_no_delete",
        "CREATE INDEX unexpected_fence_index ON remote_runtime_creation_fences(workflow_id)",
        "DROP TRIGGER remote_runtime_creation_fences_no_delete; CREATE TRIGGER remote_runtime_creation_fences_no_delete BEFORE DELETE ON remote_runtime_creation_fences BEGIN SELECT 1; END",
    ] {
        let fixture = fixture(3);
        let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
        let connection = Connection::open(fixture.store.path()).expect("raw store");
        let before = snapshots(&connection);
        connection.execute_batch(corrupt).expect("schema corruption");
        assert!(matches!(
            CloudWorkflowStore::open_path(fixture.store.path()),
            Err(CloudStoreError::InvalidCreationFenceSchema)
        ));
        assert!(matches!(
            upgraded.load(fixture.workflow.id),
            Err(CloudStoreError::InvalidCreationFenceSchema)
        ));
        assert!(matches!(
            upgraded.claim_worker_creation(
                fixture.workflow.id,
                fixture.workflow.nodes[0].id,
                &fixture.state.spec.target,
                "synthetic-worker"
            ),
            Err(CloudStoreError::InvalidCreationFenceSchema)
        ));
        assert_eq!(snapshots(&connection), before);
        let claims: i64 = connection
            .query_row("SELECT COUNT(*) FROM cloud_worker_creation_claims", [], |row| {
                row.get(0)
            })
            .expect("claims");
        assert_eq!(claims, 0);
    }
}

#[test]
fn creation_denials_are_immutable_and_lookup_uses_both_identity_indexes() {
    let fixture = fixture(3);
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
    let connection = Connection::open(upgraded.path()).expect("raw store");
    assert!(
        connection
            .execute("DELETE FROM remote_runtime_creation_fences", [])
            .is_err()
    );
    assert!(
        connection
            .execute("UPDATE remote_runtime_creation_fences SET job_id = workflow_id", [])
            .is_err()
    );
    assert_eq!(fence_count(&connection), 1);
    assert_denied(&upgraded, &fixture.workflow);
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {CLAIM_QUERY}"))
        .expect("query plan");
    let plan: Vec<String> = statement
        .query_map(
            params![
                fixture.workflow.id.to_string(),
                fixture.workflow.nodes[0].id.to_string()
            ],
            |row| row.get(3),
        )
        .expect("plan rows")
        .collect::<rusqlite::Result<_>>()
        .expect("plan");
    assert_eq!(
        plan.iter()
            .filter(|detail| detail.contains("SEARCH remote_runtime_creation_fences"))
            .count(),
        2,
        "{plan:?}"
    );
    assert!(
        plan.iter()
            .any(|detail| detail.contains("sqlite_autoindex_remote_runtime_creation_fences_1")),
        "{plan:?}"
    );
    assert!(
        plan.iter()
            .any(|detail| detail.contains("remote_runtime_creation_fences_job")),
        "{plan:?}"
    );
    assert!(
        !plan
            .iter()
            .any(|detail| detail.contains("SCAN remote_runtime_creation_fences")),
        "{plan:?}"
    );
}

#[test]
fn migration_covers_other_sessions_and_deduplicates_shared_legacy_references() {
    let fixture = fixture(3);
    let connection = Connection::open(fixture.store.path()).expect("raw store");
    let other_owner = "22222222-2222-4222-8222-222222222222";
    let mut state = fixture.state.clone();
    state.spec.workspace_local_id = "other-workspace".into();
    state
        .runtime
        .as_mut()
        .expect("runtime")
        .workspace_local_id
        .clone_from(&state.spec.workspace_local_id);
    let snapshot = serde_json::to_vec(&serde_json::json!({"session_id": other_owner, "state": state}))
        .expect("copied legacy snapshot");
    connection
        .execute(
            "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
            params![state.spec.workspace_local_id, other_owner, snapshot],
        )
        .expect("other session legacy record");
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade all sessions");
    assert_eq!(
        upgraded.list_remote_workspaces(OWNER).expect("original session").len(),
        1
    );
    assert_eq!(
        upgraded
            .list_remote_workspaces(other_owner)
            .expect("other session")
            .len(),
        1
    );
    assert_eq!(fence_count(&connection), 1);
    assert_denied(&upgraded, &fixture.workflow);
}

#[test]
fn migration_preserves_consumed_claims_and_active_snapshot_bytes() {
    let fixture = fixture(3);
    let connection = Connection::open(fixture.store.path()).expect("raw store");
    connection
        .execute(
            "INSERT INTO cloud_worker_creation_claims VALUES ('local_docker', ?1, ?2, 'synthetic-worker', 1234)",
            params![
                fixture.workflow.id.to_string(),
                fixture.workflow.nodes[0].id.to_string()
            ],
        )
        .expect("legacy consumed claim");
    let before = snapshots(&connection);
    let read_claim = || {
        connection.query_row(
        "SELECT provider, workflow_id, job_id, resource_name, claimed_at_millis FROM cloud_worker_creation_claims",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, i64>(4)?)),
    ).expect("claim")
    };
    let claim_before = read_claim();
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade consumed legacy runtime");
    assert_eq!(snapshots(&connection), before);
    assert_denied(&upgraded, &fixture.workflow);
    assert_eq!(read_claim(), claim_before);
    assert_eq!(fence_count(&connection), 1);
}

#[test]
fn schema_four_definitions_match_the_frozen_storage_format() {
    assert_eq!(
        SCHEMA,
        [
            concat!(
                "CREATE TABLE remote_runtime_creation_fences (\n",
                "    workflow_id TEXT NOT NULL CHECK (length(workflow_id) = 36),\n",
                "    job_id TEXT NOT NULL CHECK (length(job_id) = 36),\n",
                "    PRIMARY KEY (workflow_id, job_id)\n",
                ") STRICT",
            ),
            "CREATE INDEX remote_runtime_creation_fences_job ON remote_runtime_creation_fences(job_id)",
            "CREATE TRIGGER remote_runtime_creation_fences_no_update BEFORE UPDATE ON remote_runtime_creation_fences BEGIN SELECT RAISE(ABORT, 'remote creation fences are immutable'); END",
            "CREATE TRIGGER remote_runtime_creation_fences_no_delete BEFORE DELETE ON remote_runtime_creation_fences BEGIN SELECT RAISE(ABORT, 'remote creation fences are immutable'); END",
        ]
    );
}
