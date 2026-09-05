use super::*;

#[test]
fn schema_two_upgrade_preserves_existing_snapshots_and_creates_no_bindings_or_grants() {
    let (_directory, store) = store();
    let original = store
        .create_remote_workspace(OWNER, &provisioning(workspace("unbound")))
        .expect("remote record");
    let workflow = super::super::super::tests::retained_workflow(crate::cloud_run::CloudProvider::LocalDocker, 1000);
    let saved = store.create(&workflow).expect("legacy workflow");
    store
        .claim_worker_creation(
            workflow.id,
            workflow.nodes[0].id,
            workflow.nodes[0].worker.as_ref().expect("target"),
            "legacy-worker",
        )
        .expect("legacy claim");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute_batch("DROP TABLE remote_runtime_allocations; PRAGMA user_version=2;")
        .expect("schema two fixture");
    let bytes: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("original bytes");
    let upgraded = CloudWorkflowStore::open_path(store.path()).expect("upgrade");
    assert_eq!(
        upgraded.load_remote_workspace(OWNER, "unbound").expect("preserved"),
        Some(original)
    );
    assert_eq!(upgraded.load(workflow.id).expect("workflow"), Some(saved));
    assert!(matches!(
        upgraded.load_remote_allocation(OWNER, "unbound"),
        Err(Error::UnboundRuntime)
    ));
    assert_eq!(counts(&upgraded), (1, 0, 1));
    let after: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("retained bytes");
    assert_eq!(bytes, after);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("version"),
        3
    );
}

#[test]
fn missing_or_partial_allocation_schema_fails_without_recreating_ownership() {
    for sql in [
        "DROP TABLE remote_runtime_allocations",
        "DROP INDEX remote_runtime_allocations_workflow",
        "DROP INDEX remote_runtime_allocations_job",
        "PRAGMA user_version = 2",
    ] {
        let (_directory, store) = store();
        let saved = allocate(&store, &dormant(&store, "workspace"));
        let connection = Connection::open(store.path()).expect("raw store");
        connection.execute_batch(sql).expect("partial schema");
        assert!(CloudWorkflowStore::open_path(store.path()).is_err());
        assert_eq!(
            store.load(saved.workflow().workflow().id).is_ok(),
            sql != "PRAGMA user_version = 2"
        );
        let bytes: Vec<u8> = connection
            .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
            .expect("remote snapshot retained");
        assert!(!bytes.is_empty());
    }
}

#[test]
fn corrupt_allocation_metadata_blocks_recovery_and_creation_without_echoing_it() {
    for sql in [
        "UPDATE remote_runtime_allocations SET session_id='22222222-2222-4222-8222-222222222222'",
        "UPDATE remote_runtime_allocations SET generation=2",
        "UPDATE remote_runtime_allocations SET job_id='33333333-3333-4333-8333-333333333333'",
        "UPDATE remote_runtime_allocations SET session_id=hex(zeroblob(1048576))",
    ] {
        let (_directory, store) = store();
        let saved = allocate(&store, &dormant(&store, "workspace"));
        let connection = Connection::open(store.path()).expect("raw store");
        connection.execute_batch(sql).expect("corrupt index fixture");
        assert!(store.load_remote_allocation(OWNER, "workspace").is_err());
        let workflow = saved.workflow().workflow();
        let node = &workflow.nodes[0];
        let error = store
            .claim_worker_creation(
                workflow.id,
                node.id,
                node.worker.as_ref().expect("target"),
                "synthetic-worker",
            )
            .expect_err("no grant");
        assert!(error.to_string().len() < 256);
        assert_eq!(counts(&store), (1, 1, 0));
    }
}
