use super::*;

#[test]
fn corrupt_snapshot_metadata_and_exhausted_revisions_fail_closed_without_echoing_payloads() {
    let (_directory, store) = store();
    let state = workspace("workspace");
    let stored = store.create_remote_workspace(OWNER, &state).expect("create");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot = ?1",
            [b"{\"sensitive-task-payload\":true}".as_slice()],
        )
        .expect("corrupt snapshot");
    let error = store
        .load_remote_workspace(OWNER, "workspace")
        .expect_err("reject corrupt");
    assert!(matches!(error, RemoteWorkspaceStoreError::InvalidStoredSnapshot));
    assert!(!error.to_string().contains("sensitive-task-payload"));
    assert!(store.list_remote_workspaces(OWNER).is_err());
    assert!(store.replace_remote_workspace(&stored, &state).is_err());
    let altered = workspace("another-workspace");
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot = ?1",
            [encode(OWNER, &altered).expect("encode")],
        )
        .expect("identity drift");
    assert!(matches!(
        store.load_remote_workspace(OWNER, "workspace"),
        Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
    ));
    connection
        .execute(
            "UPDATE remote_workspaces SET revision = ?1, snapshot = ?2",
            params![i64::MAX, encode(OWNER, &state).expect("encode")],
        )
        .expect("exhaust revision");
    let exhausted = store
        .load_remote_workspace(OWNER, "workspace")
        .expect("load")
        .expect("record");
    assert!(matches!(
        store.replace_remote_workspace(&exhausted, &state),
        Err(RemoteWorkspaceStoreError::RevisionExhausted)
    ));
    connection
        .execute("UPDATE remote_workspaces SET revision = 1", [])
        .expect("reset revision");
    let mut same_revision_change = state.clone();
    same_revision_change.spec.working_directory = ".".into();
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot = ?1",
            [encode(OWNER, &same_revision_change).expect("encode")],
        )
        .expect("unrevisioned write");
    assert!(matches!(
        store.replace_remote_workspace(&stored, &state),
        Err(RemoteWorkspaceStoreError::SnapshotConflict)
    ));
}

#[test]
fn recovery_and_snapshot_materialization_are_bounded() {
    let (_directory, store) = store();
    store
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("create");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot = zeroblob(?1)",
            [MAX_MATERIALIZED_SNAPSHOT_BYTES],
        )
        .expect("oversize");
    assert!(matches!(
        store.load_remote_workspace(OWNER, "workspace"),
        Err(RemoteWorkspaceStoreError::SnapshotTooLarge)
    ));
    assert!(matches!(
        store.list_remote_workspaces(OWNER),
        Err(RemoteWorkspaceStoreError::SnapshotTooLarge)
    ));
    assert!(check_recovery_budget(MAX_RECOVERED_WORKSPACES + 1, 0).is_err());
    assert!(check_recovery_budget(1, MAX_RECOVERED_SNAPSHOT_BYTES + 1).is_err());
    let detail: String = connection
        .query_row(
            &format!("EXPLAIN QUERY PLAN {RECOVERY_QUERY}"),
            params![OWNER, 513, MAX_MATERIALIZED_SNAPSHOT_BYTES],
            |row| row.get(3),
        )
        .expect("query plan");
    assert!(detail.contains("USING INDEX remote_workspaces_session"));
}

#[test]
fn invalid_keys_snapshots_and_future_schema_never_replace_saved_records() {
    let (_directory, store) = store();
    let state = workspace("workspace");
    for invalid in [
        "../escape",
        "",
        "00000000-0000-0000-0000-000000000000",
        "11111111111141118111111111111111",
    ] {
        assert!(matches!(
            store.create_remote_workspace(invalid, &state),
            Err(RemoteWorkspaceStoreError::InvalidSessionId)
        ));
    }
    let stored = store.create_remote_workspace(OWNER, &state).expect("create");
    assert!(matches!(
        store.load_remote_workspace(OWNER, "workspace/escape"),
        Err(RemoteWorkspaceStoreError::InvalidWorkspaceId)
    ));
    let mut invalid = state.clone();
    invalid.version += 1;
    assert!(matches!(
        store.replace_remote_workspace(&stored, &invalid),
        Err(RemoteWorkspaceStoreError::Workspace(_))
    ));
    let connection = Connection::open(store.path()).expect("raw store");
    let before: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("before");
    connection
        .pragma_update(None, "user_version", 4)
        .expect("future schema");
    assert!(
        store
            .create_remote_workspace(OWNER, &workspace("new-workspace"))
            .is_err()
    );
    assert!(store.load_remote_workspace(OWNER, "workspace").is_err());
    assert!(store.list_remote_workspaces(OWNER).is_err());
    assert!(store.replace_remote_workspace(&stored, &state).is_err());
    assert!(CloudWorkflowStore::open_path(store.path()).is_err());
    let after: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("after");
    assert_eq!(before, after);
}

#[test]
fn indexed_owner_corruption_cannot_redirect_a_valid_snapshot() {
    let (_directory, store) = store();
    store
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("create");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute("UPDATE remote_workspaces SET session_id = ?1", [OTHER_OWNER])
        .expect("corrupt owner index");
    assert!(matches!(
        store.load_remote_workspace(OWNER, "workspace"),
        Err(RemoteWorkspaceStoreError::OwnershipMismatch)
    ));
    assert!(matches!(
        store.load_remote_workspace(OTHER_OWNER, "workspace"),
        Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
    ));
    assert!(matches!(
        store.list_remote_workspaces(OTHER_OWNER),
        Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
    ));
}

#[test]
fn oversize_writes_and_excessive_recovery_sets_are_rejected_without_partial_results() {
    let (_directory, store) = store();
    let mut huge = workspace("huge");
    let panel = huge.spec.panels[0].clone();
    huge.spec.panels = (0..65)
        .map(|index| {
            let mut panel = panel.clone();
            panel.panel_local_id = format!("panel-{index}");
            panel.task_handoff = Some("x".repeat(64 * 1024));
            panel
        })
        .collect();
    assert!(matches!(
        store.create_remote_workspace(OWNER, &huge),
        Err(RemoteWorkspaceStoreError::SnapshotTooLarge)
    ));
    assert!(
        store
            .list_remote_workspaces(OWNER)
            .expect("no partial write")
            .is_empty()
    );
    seed_workspaces(&store, std::iter::repeat_n(None, MAX_RECOVERED_WORKSPACES));
    let overflow = workspace("overflow");
    assert!(matches!(
        store.create_remote_workspace(OWNER, &overflow),
        Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded)
    ));
    assert_eq!(
        store.list_remote_workspaces(OWNER).expect("still recoverable").len(),
        MAX_RECOVERED_WORKSPACES
    );
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute(
            "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
            params![
                overflow.spec.workspace_local_id,
                OWNER,
                encode(OWNER, &overflow).expect("encode")
            ],
        )
        .expect("corrupt recovery count");
    assert!(matches!(
        store.list_remote_workspaces(OWNER),
        Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded)
    ));
    assert!(
        store
            .list_remote_workspaces(OTHER_OWNER)
            .expect("independent empty session")
            .is_empty()
    );
}

#[test]
fn concurrent_creates_cannot_overfill_the_last_recoverable_slot() {
    let (_directory, store) = store();
    seed_workspaces(&store, std::iter::repeat_n(None, MAX_RECOVERED_WORKSPACES - 1));
    let barrier = Arc::new(Barrier::new(3));
    let writers: Vec<_> = (0..2)
        .map(|index| {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.create_remote_workspace(OWNER, &workspace(&format!("new-{index}")))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().expect("writer"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded)))
            .count(),
        1
    );
    assert_eq!(
        store.list_remote_workspaces(OWNER).expect("recoverable").len(),
        MAX_RECOVERED_WORKSPACES
    );
}

#[test]
fn create_and_replace_enforce_the_serialized_session_byte_budget() {
    let (_directory, store) = store();
    seed_workspaces(
        &store,
        std::iter::repeat_n(Some(MAX_SNAPSHOT_BYTES), 15).chain([Some(MAX_SNAPSHOT_BYTES / 2); 2]),
    );
    assert!(matches!(
        store.create_remote_workspace(OWNER, &workspace("overflow")),
        Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded)
    ));
    assert_eq!(
        store
            .list_remote_workspaces(OWNER)
            .expect("exact byte budget recoverable")
            .len(),
        17
    );
    let expected = store
        .load_remote_workspace(OWNER, "workspace-0016")
        .expect("load")
        .expect("record");
    let mut grown = expected.state().clone();
    let panel = grown.spec.panels[0].clone();
    grown.spec.panels = (0..48)
        .map(|index| {
            let mut panel = panel.clone();
            panel.panel_local_id = format!("panel-{index}");
            panel.agent_session_id = None;
            panel.task_handoff = Some("x".repeat(64 * 1024));
            panel
        })
        .collect();
    assert!(matches!(
        store.replace_remote_workspace(&expected, &grown),
        Err(RemoteWorkspaceStoreError::RecoveryLimitExceeded)
    ));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace-0016").expect("unchanged"),
        Some(expected.clone())
    );
    store
        .replace_remote_workspace(&expected, expected.state())
        .expect("compacting a snapshot is allowed");
    store
        .create_remote_workspace(OWNER, &workspace("new-after-shrink"))
        .expect("capacity reclaimed by compact serialization");
}

#[test]
fn well_formed_json_cannot_bypass_aggregate_validation_during_recovery() {
    use serde_json::json;
    let (_directory, store) = store();
    let state = provisioning(workspace("workspace"));
    store.create_remote_workspace(OWNER, &state).expect("create");
    let original = serde_json::to_value(WorkspaceSnapshot {
        session_id: OWNER.into(),
        state,
    })
    .expect("value");
    let connection = Connection::open(store.path()).expect("raw store");
    for (pointer, value) in [("/state/version", json!(2)), ("/state/runtime/generation", json!(42))] {
        let mut invalid = original.clone();
        *invalid.pointer_mut(pointer).expect("field") = value;
        connection
            .execute(
                "UPDATE remote_workspaces SET snapshot = ?1",
                [serde_json::to_vec(&invalid).expect("invalid JSON")],
            )
            .expect("corrupt snapshot");
        assert!(matches!(
            store.load_remote_workspace(OWNER, "workspace"),
            Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
        ));
        assert!(matches!(
            store.list_remote_workspaces(OWNER),
            Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
        ));
    }
    let mut invalid = original;
    invalid["state"]["spec"]["workspace_local_id"] = json!("../escape");
    invalid["state"]["runtime"]["workspace_local_id"] = json!("../escape");
    connection
        .execute(
            "UPDATE remote_workspaces SET workspace_local_id = '../escape', snapshot = ?1",
            [serde_json::to_vec(&invalid).expect("invalid identity")],
        )
        .expect("corrupt indexed identity");
    assert!(matches!(
        store.list_remote_workspaces(OWNER),
        Err(RemoteWorkspaceStoreError::InvalidStoredSnapshot)
    ));
}
