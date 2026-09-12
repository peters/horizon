use super::*;
use crate::cloud_run::StoredRemoteAllocation;

const SUBSCRIPTION: &str = "00000000-0000-4000-8000-000000000001";

fn allocated() -> (Fixture, StoredRemoteAllocation) {
    let mut fixture = fixture();
    let mut state = fixture.workspace.state().clone();
    state.spec.target.provider = CloudProvider::Azure;
    fixture.workspace = fixture
        .store
        .replace_remote_workspace(&fixture.workspace, &state)
        .expect("synthetic CPU workspace");
    let allocation = fixture
        .store
        .allocate_remote_runtime(&fixture.workspace, i64::MAX)
        .expect("allocation");
    (fixture, allocation)
}

fn insert(connection: &Connection, allocation: &StoredRemoteAllocation) {
    let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
    connection
        .execute(
            "INSERT INTO remote_provider_bindings VALUES (?1, ?2, ?3, ?4, ?5, 1, 'azure', ?6, ?7)",
            params![
                allocation.workspace().state().spec.workspace_local_id,
                allocation.workspace().session_id(),
                i64::try_from(runtime.generation).expect("synthetic generation"),
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string(),
                SUBSCRIPTION,
                "a".repeat(64)
            ],
        )
        .expect("synthetic schema row, not runtime dispatch");
}

fn binding_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM remote_provider_bindings", [], |row| row.get(0))
        .expect("count")
}

#[test]
fn schema_six_reads_and_upgrade_preserve_allocations_without_backfilling_profiles() {
    let (fixture, allocation) = allocated();
    let connection = open_connection(fixture.store.path()).expect("raw store");
    connection
        .execute_batch("DROP TABLE remote_provider_bindings; PRAGMA user_version=6")
        .expect("legacy fixture");
    let before = saved_bytes(&connection);
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("legacy reader");
    assert_eq!(
        reader.load_remote_allocation(OWNER, "workspace").expect("allocation"),
        Some(allocation.clone())
    );
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("version"),
        6
    );
    assert!(matches!(
        fixture.store.validate_unclaimed_remote_setup(&allocation),
        Err(crate::cloud_run::RemoteWorkspaceStoreError::Storage(
            CloudStoreError::UnsupportedSchema(6)
        ))
    ));
    let upgraded = CloudWorkflowStore::open_path(fixture.store.path()).expect("upgrade");
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(binding_count(&connection), 0);
    assert_eq!(
        upgraded.load_remote_allocation(OWNER, "workspace").expect("allocation"),
        Some(allocation)
    );
    CloudWorkflowStore::open_path(fixture.store.path()).expect("idempotent upgrade");
    assert_eq!(binding_count(&connection), 0);
    assert_eq!(saved_bytes(&connection), before);
}

#[test]
fn provider_rows_reject_update_delete_and_insert_or_replace() {
    let (fixture, allocation) = allocated();
    let connection = open_connection(fixture.store.path()).expect("raw store");
    insert(&connection, &allocation);
    let before = saved_bytes(&connection);
    for sql in [
        "UPDATE remote_provider_bindings SET subscription_id = '00000000-0000-4000-8000-000000000002'",
        "UPDATE remote_provider_bindings SET profile_digest = lower(hex(randomblob(32)))",
        "DELETE FROM remote_provider_bindings",
        "INSERT OR REPLACE INTO remote_provider_bindings SELECT * FROM remote_provider_bindings",
        "INSERT INTO remote_provider_bindings SELECT * FROM remote_provider_bindings WHERE true ON CONFLICT DO NOTHING",
    ] {
        assert_eq!(
            connection
                .execute_batch(sql)
                .expect_err("immutable binding")
                .sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation),
            "{sql}"
        );
        assert_eq!(binding_count(&connection), 1);
        assert_eq!(saved_bytes(&connection), before);
        let subscription: String = connection
            .query_row("SELECT subscription_id FROM remote_provider_bindings", [], |row| {
                row.get(0)
            })
            .expect("original subscription");
        assert_eq!(subscription, SUBSCRIPTION);
    }
    CloudWorkflowStore::open_path(fixture.store.path()).expect("reopen with retained metadata");
}

#[test]
fn malformed_provider_schema_fails_without_repair_or_snapshot_changes() {
    for sql in [
        "DROP TABLE remote_provider_bindings",
        "DROP TRIGGER remote_provider_bindings_no_replace",
        "DROP TRIGGER remote_provider_bindings_no_update",
        "DROP TRIGGER remote_provider_bindings_no_delete",
        "CREATE INDEX unexpected_provider_index ON remote_provider_bindings(subscription_id)",
        "CREATE INDEX remote_provider_bindings_orphan ON cloud_workflows(created_at_millis)",
        "ALTER TABLE remote_provider_bindings ADD COLUMN unexpected TEXT",
    ] {
        let (fixture, allocation) = allocated();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        let before = saved_bytes(&connection);
        connection.execute_batch(sql).expect("corrupt schema fixture");
        assert!(matches!(
            CloudWorkflowStore::open_path(fixture.store.path()),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        assert!(CloudWorkflowStore::open_read_only_path(fixture.store.path()).is_err());
        assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
        assert_eq!(saved_bytes(&connection), before);
        assert_eq!(allocation_count(&connection), 1);
        assert_eq!(allocation.workspace().state().spec.generation, 1);
    }
}

#[test]
fn legacy_partial_provider_objects_do_not_gain_a_binding_or_schema_upgrade() {
    for (version, orphan) in [(4, false), (5, false), (6, false), (4, true), (5, true), (6, true)] {
        let (fixture, _) = allocated();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        if orphan {
            connection
                .execute_batch("DROP TABLE remote_provider_bindings; CREATE INDEX remote_provider_bindings_orphan ON cloud_workflows(created_at_millis)")
                .expect("isolated partial namespace");
        }
        if version < 6 {
            connection
                .execute_batch("DROP TABLE remote_network_volume_selections")
                .expect("v5");
        }
        if version == 4 {
            connection
                .execute_batch("DROP TABLE remote_first_pin_intents")
                .expect("v4");
        }
        connection
            .pragma_update(None, "user_version", version)
            .expect("legacy version");
        let before = saved_bytes(&connection);
        assert!(CloudWorkflowStore::open_read_only_path(fixture.store.path()).is_err());
        assert!(CloudWorkflowStore::open_path(fixture.store.path()).is_err());
        assert_eq!(saved_bytes(&connection), before);
        if orphan {
            let table_exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='remote_provider_bindings')",
                    [],
                    |row| row.get(0),
                )
                .expect("no table repair");
            assert!(!table_exists);
        } else {
            assert_eq!(binding_count(&connection), 0);
        }
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged"),
            version
        );
    }
}

#[test]
fn provider_binding_constraints_and_exact_sql_are_enforced() {
    for (column, value) in [
        ("session_id", "'short'"),
        ("generation", "0"),
        ("version", "2"),
        ("provider", "'run_pod'"),
        ("subscription_id", "'short'"),
        ("profile_digest", "'short'"),
        ("profile_digest", "printf('%064d', 0) || char(0)"),
    ] {
        let (fixture, allocation) = allocated();
        let connection = open_connection(fixture.store.path()).expect("raw store");
        insert(&connection, &allocation);
        let columns = [
            "workspace_local_id",
            "session_id",
            "generation",
            "workflow_id",
            "job_id",
            "version",
            "provider",
            "subscription_id",
            "profile_digest",
        ];
        let projection = columns.map(|name| if name == column { value } else { name }).join(", ");
        connection
            .execute_batch("DROP TRIGGER remote_provider_bindings_no_replace")
            .expect("constraint isolation");
        let sql = format!(
            "INSERT OR REPLACE INTO remote_provider_bindings SELECT {projection} FROM remote_provider_bindings"
        );
        assert_eq!(
            connection
                .execute_batch(&sql)
                .expect_err("binding constraint")
                .sqlite_error_code(),
            Some(rusqlite::ErrorCode::ConstraintViolation),
            "{column}"
        );
        assert_eq!(binding_count(&connection), 1);
    }
    let fixture = fixture();
    let connection = open_connection(fixture.store.path()).expect("raw store");
    for definition in PROVIDER_BINDING_SCHEMA {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE sql = ?1)",
                [definition],
                |row| row.get(0),
            )
            .expect("frozen SQL");
        assert!(exists);
    }
}
