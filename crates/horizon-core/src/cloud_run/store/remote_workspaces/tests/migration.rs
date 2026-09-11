use super::*;

#[test]
fn schema_one_migration_preserves_workflow_bytes_revisions_and_creation_claims() {
    let (_directory, store) = store();
    let workflow = super::super::super::tests::retained_workflow(CloudProvider::RunPod, 1000);
    let saved = store.create(&workflow).expect("save legacy workflow");
    let target = workflow.nodes[0].worker.as_ref().expect("worker target");
    store
        .claim_worker_creation(workflow.id, workflow.nodes[0].id, target, "legacy-worker")
        .expect("legacy creation fence");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute_batch("DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; DROP TABLE remote_runtime_creation_fences; DROP TABLE remote_runtime_allocations; DROP TABLE remote_workspaces; PRAGMA user_version = 1;")
        .expect("restore schema-one fixture");
    let workflow_bytes: Vec<u8> = connection
        .query_row("SELECT snapshot FROM cloud_workflows", [], |row| row.get(0))
        .expect("legacy snapshot bytes");
    let claim: (String, String, String, String, i64) = connection
        .query_row(
            "SELECT provider, workflow_id, job_id, resource_name, claimed_at_millis FROM cloud_worker_creation_claims",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .expect("legacy claim");
    drop(connection);

    let upgraded = CloudWorkflowStore::open_path(store.path()).expect("upgrade schema one");
    assert_eq!(upgraded.load(workflow.id).expect("load legacy workflow"), Some(saved));
    assert!(
        !upgraded
            .claim_worker_creation(workflow.id, workflow.nodes[0].id, target, "legacy-worker")
            .expect("fence remains claimed")
    );
    assert!(
        upgraded
            .list_remote_workspaces(OWNER)
            .expect("empty remote records")
            .is_empty()
    );
    let connection = Connection::open(upgraded.path()).expect("raw upgraded store");
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("schema version");
    assert_eq!(version, 6);
    let after: Vec<u8> = connection
        .query_row("SELECT snapshot FROM cloud_workflows", [], |row| row.get(0))
        .expect("unchanged legacy bytes");
    assert_eq!(workflow_bytes, after);
    let after_claim: (String, String, String, String, i64) = connection
        .query_row(
            "SELECT provider, workflow_id, job_id, resource_name, claimed_at_millis FROM cloud_worker_creation_claims",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .expect("unchanged legacy claim");
    assert_eq!(claim, after_claim);
    upgraded
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("new remote snapshot");
    assert_eq!(
        CloudWorkflowStore::open_path(upgraded.path())
            .expect("repeat open")
            .list_remote_workspaces(OWNER)
            .expect("retained remote record")
            .len(),
        1
    );
}

#[test]
fn partial_migration_fails_without_adopting_or_destroying_existing_remote_data() {
    let (_directory, store) = store();
    let state = workspace("workspace");
    store.create_remote_workspace(OWNER, &state).expect("create");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .pragma_update(None, "user_version", 1)
        .expect("partial migration fixture");
    assert!(CloudWorkflowStore::open_path(store.path()).is_err());
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version unchanged");
    assert_eq!(version, 1);
    let bytes: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("snapshot retained");
    assert_eq!(
        serde_json::from_slice::<WorkspaceSnapshot<RemoteWorkspaceState>>(&bytes)
            .expect("valid record")
            .state,
        state
    );
}

#[test]
fn current_schema_with_missing_remote_table_or_index_fails_at_open() {
    for sql in ["DROP TABLE remote_workspaces", "DROP INDEX remote_workspaces_session"] {
        let (_directory, store) = store();
        let connection = Connection::open(store.path()).expect("raw store");
        connection.execute_batch(sql).expect("incomplete schema fixture");
        assert!(CloudWorkflowStore::open_path(store.path()).is_err());
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("schema unchanged");
        assert_eq!(version, 6);
    }
}

use crate::cloud_run::RemoteWorkspaceStoreError as Error;
use crate::cloud_run::{StoredRemoteAllocation, runpod::RunPodNetworkVolumeExpectation};
use base64::{Engine as _, engine::general_purpose::STANDARD};
const PRIVATE: &str = "private-sentinel-not-selection-data";

struct SelectionFixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    saved: StoredRemoteAllocation,
}
impl SelectionFixture {
    fn new() -> Self {
        Self::with_provider("run_pod")
    }
    fn with_provider(provider: &str) -> Self {
        let (directory, store) = store();
        let mut state = workspace("workspace");
        state.spec.panels.clear();
        state.spec.target.provider = serde_json::from_value(serde_json::json!(provider)).expect("provider");
        state.spec.target.lifetime = WorkerLifetime::Persistent;
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
    fn snapshots(&self) -> Vec<(i64, Vec<u8>)> {
        self.raw().prepare("SELECT revision, snapshot FROM remote_workspaces UNION ALL SELECT revision, snapshot FROM cloud_workflows")
            .expect("query").query_map([], |row| Ok((row.get(0)?, row.get(1)?))).expect("rows")
            .collect::<Result<_, _>>().expect("snapshots")
    }
    fn count(&self) -> i64 {
        self.raw()
            .query_row("SELECT COUNT(*) FROM remote_network_volume_selections", [], |row| {
                row.get(0)
            })
            .expect("count")
    }
    fn reserve(&self) -> StoredRemoteAllocation {
        let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        key.extend([7; 32]);
        self.store
            .reserve_remote_worker_request(&self.saved, &format!("ssh-ed25519 {}", STANDARD.encode(key)))
            .expect("key")
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
fn invalid_selection_and_mutation_or_corrupt_rows_fail_without_repair() {
    let unsupported = SelectionFixture::with_provider("local_docker");
    assert!(matches!(
        unsupported
            .store
            .record_remote_network_volume_selection(&unsupported.saved, &selection()),
        Err(Error::RuntimeSetupUnavailable)
    ));
    assert_eq!(unsupported.count(), 0);
    for chosen in [
        RunPodNetworkVolumeExpectation {
            volume_id: format!("../{PRIVATE}"),
            ..selection()
        },
        RunPodNetworkVolumeExpectation {
            data_center_id: "x".repeat(192),
            ..selection()
        },
        RunPodNetworkVolumeExpectation {
            minimum_size_gb: 9,
            ..selection()
        },
        RunPodNetworkVolumeExpectation {
            minimum_size_gb: 4_097,
            ..selection()
        },
    ] {
        let fixture = SelectionFixture::new();
        let error = fixture
            .store
            .record_remote_network_volume_selection(&fixture.saved, &chosen)
            .expect_err("invalid");
        assert!(matches!(error, Error::InvalidNetworkVolumeSelection));
        assert!(!error.to_string().contains(PRIVATE));
        assert_eq!(fixture.count(), 0);
    }
    let fixture = SelectionFixture::new();
    fixture
        .store
        .record_remote_network_volume_selection(&fixture.saved, &selection())
        .expect("record");
    for sql in [
        "UPDATE remote_network_volume_selections SET minimum_size_gb=11",
        "DELETE FROM remote_network_volume_selections",
    ] {
        assert!(fixture.raw().execute_batch(sql).is_err());
    }
    for sql in [
        "UPDATE remote_network_volume_selections SET volume_id='../foreign'",
        "UPDATE remote_network_volume_selections SET volume_id='volume_exact'||char(0)||'foreign'",
        "UPDATE remote_network_volume_selections SET data_center_id='EU-TEST-1'||char(0)||'foreign'",
        "UPDATE remote_network_volume_selections SET session_id='22222222-2222-4222-8222-222222222222'",
        "UPDATE remote_network_volume_selections SET generation=2",
        "UPDATE remote_network_volume_selections SET workflow_id='22222222-2222-4222-8222-222222222222'",
        "UPDATE remote_network_volume_selections SET job_id='22222222-2222-4222-8222-222222222222'",
        "UPDATE remote_network_volume_selections SET storage_type='STANDARD'",
        "UPDATE remote_network_volume_selections SET version=2",
        "UPDATE remote_network_volume_selections SET workspace_local_id='orphan'",
        "UPDATE remote_network_volume_selections SET volume_id=printf('%02000d', 1)",
    ] {
        let fixture = SelectionFixture::new();
        fixture
            .store
            .record_remote_network_volume_selection(&fixture.saved, &selection())
            .expect("record");
        let raw = fixture.raw();
        let trigger: String = raw
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                [],
                |row| row.get(0),
            )
            .expect("trigger");
        raw.execute_batch(
            "DROP TRIGGER remote_network_volume_selections_no_update; PRAGMA ignore_check_constraints=ON; PRAGMA foreign_keys=OFF",
        )
        .expect("corruption fixture");
        raw.execute_batch(sql).expect("corrupt");
        raw.execute_batch(&trigger).expect("restore exact schema");
        assert!(matches!(
            fixture.store.load_remote_network_volume_selection(&fixture.saved),
            Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
        ));
        assert!(matches!(
            fixture
                .store
                .record_remote_network_volume_selection(&fixture.saved, &selection()),
            Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
        ));
        assert_eq!(fixture.count(), 1);
    }
}

#[test]
fn legacy_inventory_is_read_only_and_migration_never_backfills_a_selection() {
    for version in [4, 5] {
        let fixture = SelectionFixture::new();
        let saved = fixture.reserve();
        if version == 5 {
            fixture.store.record_remote_first_pin_intent(&saved).expect("first pin");
        }
        let raw = fixture.raw();
        raw.execute_batch("DROP TABLE remote_network_volume_selections")
            .expect("legacy schema");
        if version == 4 {
            raw.execute_batch("DROP TABLE remote_first_pin_intents").expect("v4");
        }
        raw.pragma_update(None, "user_version", version).expect("version");
        let before = fixture.snapshots();
        let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("legacy reader");
        assert_eq!(
            reader
                .load_remote_network_volume_selection(&saved)
                .expect("legacy selection"),
            None
        );
        assert_eq!(
            reader.list_remote_environment_page(None).expect("inventory").records,
            vec![saved.workspace().clone()]
        );
        assert_eq!(
            reader
                .load_remote_first_pin_request(&saved)
                .expect("first pin")
                .is_some(),
            version == 5
        );
        assert_eq!(
            raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            version
        );
        assert_eq!(fixture.snapshots(), before);
        let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
        assert_eq!(
            upgraded
                .load_remote_network_volume_selection(&saved)
                .expect("unselected"),
            None
        );
        assert_eq!(fixture.count(), 0);
        assert_eq!(fixture.snapshots(), before);
        assert_eq!(
            raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("version"),
            6
        );
    }
}

#[test]
fn partial_or_missing_schema_and_missing_database_never_trigger_repair_on_reads() {
    for version in [4, 5, 6] {
        let fixture = SelectionFixture::new();
        let raw = fixture.raw();
        if version == 6 {
            raw.execute_batch("DROP TRIGGER remote_network_volume_selections_no_delete")
                .expect("partial");
        }
        raw.pragma_update(None, "user_version", version).expect("version");
        assert!(CloudWorkflowStore::open_read_only_path(fixture.store.path()).is_err());
        assert!(
            fixture
                .store
                .load_remote_network_volume_selection(&fixture.saved)
                .is_err()
        );
        assert!(CloudWorkflowStore::open_path(fixture.store.path()).is_err());
        assert_eq!(
            raw.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged"),
            version
        );
    }
    let fixture = SelectionFixture::new();
    std::fs::rename(fixture.store.path(), fixture.store.path().with_extension("saved")).expect("move task database");
    assert!(
        fixture
            .store
            .load_remote_network_volume_selection(&fixture.saved)
            .is_err()
    );
    assert!(!fixture.store.path().exists());
}
