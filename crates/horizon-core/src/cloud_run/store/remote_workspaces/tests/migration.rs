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
        .execute_batch("DROP TABLE remote_runtime_creation_fences; DROP TABLE remote_runtime_allocations; DROP TABLE remote_workspaces; PRAGMA user_version = 1;")
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
    assert_eq!(version, 4);
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
        assert_eq!(version, 4);
    }
}
