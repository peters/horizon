use super::*;
use rusqlite::params;

#[test]
fn missing_binding_cannot_enable_workspace_retirement_or_adoption() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    Connection::open(store.path())
        .expect("raw store")
        .execute("DELETE FROM remote_runtime_allocations", [])
        .expect("lost binding fixture");
    let mut cleared = saved.workspace().state().clone();
    cleared.runtime = None;
    assert!(matches!(
        store.replace_remote_workspace(saved.workspace(), &cleared),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    let mut copied = saved.workspace().state().clone();
    copied.spec.workspace_local_id = "copy".into();
    copied.runtime.as_mut().expect("runtime").workspace_local_id = "copy".into();
    assert!(matches!(
        store.create_remote_workspace(OTHER_OWNER, &copied),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    assert_eq!(
        store
            .load_remote_workspace(OWNER, "workspace")
            .expect("original retained"),
        Some(saved.workspace().clone())
    );
    assert!(
        store
            .load_remote_workspace(OTHER_OWNER, "copy")
            .expect("no copy")
            .is_none()
    );
    assert_eq!(counts(&store), (1, 0, 0));
}

#[test]
fn missing_binding_is_corruption_not_permission_to_reallocate_or_reclassify() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    Connection::open(store.path())
        .expect("raw store")
        .execute("DELETE FROM remote_runtime_allocations", [])
        .expect("lost binding fixture");
    let workflow = saved.workflow().workflow();
    assert!(matches!(
        store.claim_worker_creation(
            workflow.id,
            workflow.nodes[0].id,
            workflow.nodes[0].worker.as_ref().expect("target"),
            "synthetic-worker"
        ),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    for kind in [WorkflowNodeKind::RemoteWorkspace, WorkflowNodeKind::Build] {
        let mut next = workflow.clone();
        next.updated_at_millis += 1;
        next.nodes[0].kind = kind;
        assert!(matches!(
            store.replace(saved.workflow(), &next),
            Err(CloudStoreError::InvalidRemoteAllocation)
        ));
    }
    assert_eq!(
        store.load(workflow.id).expect("snapshot retained"),
        Some(saved.workflow().clone())
    );
    assert_eq!(counts(&store), (1, 0, 0));
}

#[test]
fn allocation_checks_the_exact_snapshot_even_when_revision_did_not_change() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute_batch(
            "UPDATE remote_workspaces SET snapshot = CAST(json_set(CAST(snapshot AS TEXT),
                '$.state.spec.working_directory', 'other') AS BLOB)",
        )
        .expect("same-revision drift");
    assert!(matches!(
        store.allocate_remote_runtime(&original, 1000, 901_000),
        Err(Error::SnapshotConflict)
    ));
    assert_eq!(counts(&store), (0, 0, 0));
    let current = store
        .load_remote_workspace(OWNER, "workspace")
        .expect("load")
        .expect("record");
    assert_eq!(current.revision(), original.revision());
    assert_eq!(current.state().spec.working_directory, "other");
}

#[test]
fn allocation_cannot_grow_a_full_session_past_its_recovery_budget() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    let mut connection = Connection::open(store.path()).expect("raw store");
    let original_bytes: i64 = connection
        .query_row("SELECT length(snapshot) FROM remote_workspaces", [], |row| row.get(0))
        .expect("original size");
    let original_bytes = usize::try_from(original_bytes).expect("bounded size");
    let transaction = connection.transaction().expect("fixture transaction");
    for index in 0..16 {
        let state = workspace(&format!("filled-{index}"));
        let mut snapshot =
            serde_json::to_vec(&serde_json::json!({"session_id": OWNER, "state": state})).expect("valid snapshot");
        let size = 4 * 1024 * 1024 - if index == 15 { original_bytes } else { 0 };
        snapshot.resize(size, b' ');
        transaction
            .execute(
                "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
                params![state.spec.workspace_local_id, OWNER, snapshot],
            )
            .expect("budget fixture");
    }
    transaction.commit().expect("commit fixture");
    assert_eq!(
        store
            .list_remote_workspaces(OWNER)
            .expect("full session remains recoverable")
            .len(),
        17
    );
    assert!(matches!(
        store.allocate_remote_runtime(&original, 1000, 901_000),
        Err(Error::RecoveryLimitExceeded)
    ));
    assert_eq!(counts(&store), (0, 0, 0));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(original)
    );
}

#[test]
fn missing_binding_or_corrupt_cross_record_identity_never_grants_creation() {
    for sql in [
        "DELETE FROM remote_runtime_allocations",
        "DELETE FROM cloud_workflows",
        "DELETE FROM remote_workspaces",
        "UPDATE remote_runtime_allocations SET workflow_id='33333333-3333-4333-8333-333333333333'",
        "UPDATE cloud_workflows SET snapshot = CAST(json_set(CAST(snapshot AS TEXT),
            '$.nodes[0].source.repository', 'other/project') AS BLOB)",
        "UPDATE cloud_workflows SET snapshot = CAST(json_set(CAST(snapshot AS TEXT),
            '$.nodes[0].worker.disk_gib', 21) AS BLOB)",
    ] {
        let (_directory, store) = store();
        let saved = allocate(&store, &dormant(&store, "workspace"));
        let workflow = saved.workflow().workflow();
        let connection = Connection::open(store.path()).expect("raw store");
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("allow synthetic corruption");
        connection.execute_batch(sql).expect("corruption");
        assert!(store.load_remote_allocation(OWNER, "workspace").is_err(), "{sql}");
        assert!(
            store
                .claim_worker_creation(
                    workflow.id,
                    workflow.nodes[0].id,
                    workflow.nodes[0].worker.as_ref().expect("target"),
                    "synthetic-worker"
                )
                .is_err(),
            "{sql}"
        );
        assert_eq!(counts(&store).2, 0);
    }
}

#[test]
fn separate_workspaces_keep_distinct_jobs_and_expired_allocations_remain_recoverable() {
    let (_directory, store) = store();
    let first = store
        .allocate_remote_runtime(&dormant(&store, "one"), 1000, 901_000)
        .expect("first");
    let second = store
        .allocate_remote_runtime(&dormant(&store, "two"), 1000, 901_000)
        .expect("second");
    assert_ne!(first.workflow().workflow().id, second.workflow().workflow().id);
    assert_ne!(
        first.workflow().workflow().nodes[0].id,
        second.workflow().workflow().nodes[0].id
    );
    for saved in [first, second] {
        let workflow = saved.workflow().workflow();
        assert!(matches!(
            store.claim_worker_creation(
                workflow.id,
                workflow.nodes[0].id,
                workflow.nodes[0].worker.as_ref().expect("target"),
                "synthetic-worker"
            ),
            Err(CloudStoreError::WorkflowExpired(_))
        ));
        assert_eq!(
            store
                .load_remote_allocation(OWNER, &saved.workspace().state().spec.workspace_local_id)
                .expect("expired recovery"),
            Some(saved)
        );
    }
    assert_eq!(counts(&store), (2, 2, 0));
}

#[test]
fn allocation_cannot_overfill_retained_workflow_count_or_byte_budgets() {
    for (count, snapshot_size) in [(512, None), (16, Some(4 * 1024 * 1024))] {
        let (_directory, store) = store();
        let original = dormant(&store, "workspace");
        let mut connection = Connection::open(store.path()).expect("raw store");
        let transaction = connection.transaction().expect("fixture transaction");
        for _ in 0..count {
            let workflow =
                super::super::super::tests::retained_workflow(crate::cloud_run::CloudProvider::LocalDocker, 1000);
            let mut snapshot = serde_json::to_vec(&workflow).expect("valid workflow");
            if let Some(size) = snapshot_size {
                snapshot.resize(size, b' ');
            }
            transaction
                .execute(
                    "INSERT INTO cloud_workflows VALUES (?1, 1, ?2, ?2, ?3, ?4)",
                    params![
                        workflow.id.to_string(),
                        workflow.created_at_millis,
                        workflow.retain_until_millis,
                        snapshot
                    ],
                )
                .expect("retained workflow fixture");
        }
        transaction.commit().expect("commit fixture");
        assert_eq!(store.list_retained(1000).expect("full set is recoverable").len(), count);
        let now = current_unix_millis().expect("timestamp");
        assert!(matches!(
            store.allocate_remote_runtime(&original, now, now + 86_400_000),
            Err(Error::Storage(CloudStoreError::RecoveryLimitExceeded))
        ));
        assert_eq!(counts(&store), (i64::try_from(count).expect("count"), 0, 0));
        assert_eq!(
            store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
            Some(original)
        );
    }
}
