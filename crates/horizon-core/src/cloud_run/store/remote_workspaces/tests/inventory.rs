use super::*;

#[test]
fn inventory_pages_all_owners_without_client_sessions_and_survives_reopen() {
    let (_directory, store) = store();
    let mut expected = Vec::new();
    for index in (0..35).rev() {
        let owner = if index % 2 == 0 { OWNER } else { OTHER_OWNER };
        let mut state = workspace(&format!("workspace-{index:04}"));
        state.spec.panels.clear();
        expected.push(store.create_remote_workspace(owner, &state).expect("record"));
    }
    expected.sort_by(|left, right| {
        left.state()
            .spec
            .workspace_local_id
            .cmp(&right.state().spec.workspace_local_id)
    });
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen without client sessions");
    let mut cursor = None;
    let mut recovered = Vec::new();
    for count in [16, 16, 3] {
        let page = reopened.list_remote_environment_page(cursor.as_deref()).expect("page");
        assert_eq!(page.records.len(), count);
        cursor = page.next_cursor;
        recovered.extend(page.records);
    }
    assert!(cursor.is_none());
    assert_eq!(recovered, expected);
    assert!(recovered.iter().all(|record| record.revision() == 1));
}

#[test]
fn inventory_empty_exact_page_and_terminal_cursor_are_unambiguous() {
    let (_directory, store) = store();
    let page = store.list_remote_environment_page(None).expect("empty store");
    assert!(page.records.is_empty());
    assert!(page.next_cursor.is_none());
    seed_workspaces(&store, std::iter::repeat_n(None, 16));
    let page = store.list_remote_environment_page(None).expect("exact page");
    assert_eq!(page.records.len(), 16);
    assert!(page.next_cursor.is_none());
    for cursor in ["workspace-0015", "zz-missing"] {
        let page = store.list_remote_environment_page(Some(cursor)).expect("after last");
        assert!(page.records.is_empty());
        assert!(page.next_cursor.is_none());
    }
}

#[test]
fn inventory_keyset_does_not_replay_earlier_rows_when_records_are_added() {
    let (_directory, store) = store();
    seed_workspaces(&store, std::iter::repeat_n(None, 17));
    let first = store.list_remote_environment_page(None).expect("first page");
    assert_eq!(first.next_cursor.as_deref(), Some("workspace-0015"));
    store
        .create_remote_workspace(OTHER_OWNER, &workspace("a-new-record"))
        .expect("earlier insertion");
    store
        .create_remote_workspace(OTHER_OWNER, &workspace("z-new-record"))
        .expect("later insertion");
    let second = store
        .list_remote_environment_page(first.next_cursor.as_deref())
        .expect("next page");
    let ids: Vec<_> = second
        .records
        .iter()
        .map(|record| record.state().spec.workspace_local_id.as_str())
        .collect();
    assert_eq!(ids, ["workspace-0016", "z-new-record"]);
    assert!(second.next_cursor.is_none());
    assert_eq!(
        store.list_remote_environment_page(None).expect("refresh").records[0]
            .state()
            .spec
            .workspace_local_id,
        "a-new-record"
    );
}

#[test]
fn inventory_keeps_dormant_failed_and_expired_legacy_records_without_mutation() {
    let (_directory, store) = store();
    let dormant = store
        .create_remote_workspace(OTHER_OWNER, &workspace("a-dormant"))
        .expect("dormant");
    let mut failed = provisioning(workspace("b-failed"));
    failed.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Failed;
    let failed = seed_legacy_workspace(&store, &failed);
    let mut legacy = ownership::expired_runtime();
    legacy.spec.workspace_local_id = "c-expired".into();
    legacy.runtime.as_mut().expect("runtime").workspace_local_id = "c-expired".into();
    let legacy = seed_legacy_workspace(&store, &legacy);
    let connection = Connection::open(store.path()).expect("raw store");
    let before: Vec<(String, i64, Vec<u8>)> = connection
        .prepare("SELECT workspace_local_id, revision, snapshot FROM remote_workspaces ORDER BY workspace_local_id")
        .expect("before query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("before rows")
        .collect::<Result<_, _>>()
        .expect("before");
    assert_eq!(
        store.list_remote_environment_page(None).expect("inventory").records,
        [dormant, failed, legacy]
    );
    let after: Vec<(String, i64, Vec<u8>)> = connection
        .prepare("SELECT workspace_local_id, revision, snapshot FROM remote_workspaces ORDER BY workspace_local_id")
        .expect("after query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("after rows")
        .collect::<Result<_, _>>()
        .expect("after");
    assert_eq!(before, after);
}

#[test]
fn inventory_rejects_invalid_cursors_before_opening_the_store() {
    let (_directory, store) = store();
    Connection::open(store.path())
        .expect("raw store")
        .pragma_update(None, "user_version", 999)
        .expect("future schema");
    for invalid in ["", "../escape", "workspace/escape", "new\nline", &"x".repeat(129)] {
        assert!(matches!(
            store.list_remote_environment_page(Some(invalid)),
            Err(RemoteWorkspaceStoreError::InvalidWorkspaceId)
        ));
    }
    assert!(matches!(
        store.list_remote_environment_page(None),
        Err(RemoteWorkspaceStoreError::Storage(CloudStoreError::UnsupportedSchema(
            999
        )))
    ));
}

#[test]
fn inventory_fails_closed_on_selected_corruption_without_echoing_content() {
    for corruption in [
        "UPDATE remote_workspaces SET session_id = 'invalid-owner' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET session_id = '22222222-2222-4222-8222-222222222222' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET workspace_local_id = 'invalid/id' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET workspace_local_id = '' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET workspace_local_id = workspace_local_id || char(0) || 'tail' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET session_id = session_id || char(0) || 'tail' WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET snapshot = CAST('sensitive-task-payload' AS BLOB) WHERE workspace_local_id = 'workspace-0001'",
        "UPDATE remote_workspaces SET snapshot = zeroblob(4194305) WHERE workspace_local_id = 'workspace-0001'",
    ] {
        let (_directory, store) = store();
        seed_workspaces(&store, [None; 2]);
        Connection::open(store.path())
            .expect("raw store")
            .execute_batch(corruption)
            .expect("corrupt fixture");
        let error = store.list_remote_environment_page(None).expect_err("no partial page");
        assert!(!error.to_string().contains("sensitive-task-payload"));
    }
}

#[test]
fn inventory_does_not_materialize_or_validate_records_beyond_its_page() {
    let (_directory, store) = store();
    seed_workspaces(&store, std::iter::repeat_n(Some(MAX_SNAPSHOT_BYTES), 16).chain([None]));
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot = zeroblob(?1) WHERE workspace_local_id = 'workspace-0016'",
            [MAX_MATERIALIZED_SNAPSHOT_BYTES],
        )
        .expect("oversize next page");
    let page = store.list_remote_environment_page(None).expect("maximum-size page");
    assert_eq!(page.records.len(), 16);
    assert_eq!(page.next_cursor.as_deref(), Some("workspace-0015"));
    assert!(matches!(
        store.list_remote_environment_page(page.next_cursor.as_deref()),
        Err(RemoteWorkspaceStoreError::SnapshotTooLarge)
    ));
}
