use super::*;

#[test]
fn single_record_reads_never_recreate_a_missing_database() {
    for allocation in [false, true] {
        let fixture = fixture();
        let path = fixture.store.path();
        let retained = path.with_file_name("retained.sqlite3");
        std::fs::rename(path, &retained).expect("retain database");
        let saved = std::fs::read(&retained).expect("retained bytes");
        if allocation {
            assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
        } else {
            assert!(fixture.store.load_remote_workspace(OWNER, "workspace").is_err());
        }
        assert!(!path.exists(), "a read must not recreate the database");
        assert_eq!(std::fs::read(retained).expect("preserved database"), saved);
    }
}

#[test]
fn read_only_open_never_creates_database_or_parent_directories() {
    let directory = tempfile::tempdir().expect("private directory");
    let parent = directory.path().join("absent");
    let path = parent.join("workflows.sqlite3");
    assert!(CloudWorkflowStore::open_read_only_path(&path).is_err());
    assert!(!parent.exists());
    std::fs::create_dir(&parent).expect("existing parent");
    assert!(CloudWorkflowStore::open_read_only_path(&path).is_err());
    assert_eq!(std::fs::read_dir(parent).expect("unchanged parent").count(), 0);
}

#[test]
fn read_only_clones_keep_inventory_noncreating_after_open() {
    let fixture = fixture();
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    let reader = reader.clone();
    assert_eq!(
        reader.list_remote_workspaces(OWNER).expect("inventory"),
        [fixture.workspace]
    );
    let parent = fixture.store.path().parent().expect("control directory");
    let retained = parent.with_file_name("retained-control");
    std::fs::rename(parent, &retained).expect("retain whole directory");
    assert!(reader.list_remote_workspaces(OWNER).is_err());
    assert!(reader.load_remote_workspace(OWNER, "workspace").is_err());
    assert!(reader.load_remote_allocation(OWNER, "workspace").is_err());
    assert!(!parent.exists());
    assert!(retained.join("workflows.sqlite3").is_file());
}

#[test]
fn read_only_handle_rejects_writes_and_preserves_stored_records() {
    let fixture = fixture();
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    let connection = fixture.store.connection().expect("writer");
    let before = saved_bytes(&connection);
    assert!(
        reader
            .connection()
            .expect("read connection")
            .is_readonly("main")
            .expect("mode")
    );
    assert!(
        reader
            .connection()
            .expect("read connection")
            .execute("DELETE FROM remote_workspaces", [])
            .is_err()
    );
    assert!(reader.allocate_remote_runtime(&fixture.workspace, i64::MAX).is_err());
    assert_eq!(saved_bytes(&connection), before);
    assert_eq!(allocation_count(&connection), 0);
    assert_eq!(
        reader.load_remote_workspace(OWNER, "workspace").expect("workspace"),
        Some(fixture.workspace)
    );
}

#[test]
fn readers_observe_committed_wal_updates_without_mutating_records() {
    let fixture = fixture();
    let mut writer = fixture.store.connection().expect("live writer");
    let allocation = fixture
        .store
        .allocate_remote_runtime(&fixture.workspace, i64::MAX)
        .expect("allocation");
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    let database_before = std::fs::read(fixture.store.path()).expect("database bytes");
    assert_eq!(
        reader.load_remote_allocation(OWNER, "workspace").expect("allocation"),
        Some(allocation.clone())
    );
    let transaction = writer.transaction().expect("uncommitted writer");
    transaction
        .execute(
            "UPDATE remote_workspaces SET revision = revision + 1 WHERE workspace_local_id = 'workspace'",
            [],
        )
        .expect("new revision");
    assert_eq!(
        reader
            .load_remote_allocation(OWNER, "workspace")
            .expect("committed allocation only"),
        Some(allocation.clone())
    );
    transaction.commit().expect("commit update to WAL");
    let updated = reader
        .load_remote_allocation(OWNER, "workspace")
        .expect("fresh allocation")
        .expect("record");
    assert_eq!(updated.workspace().revision(), allocation.workspace().revision() + 1);
    assert_eq!(updated.workspace().state(), allocation.workspace().state());
    assert_eq!(updated.workflow(), allocation.workflow());
    assert_eq!(
        reader
            .load_remote_workspace(OWNER, "workspace")
            .expect("fresh workspace"),
        Some(updated.workspace().clone())
    );
    assert_eq!(
        std::fs::read(fixture.store.path()).expect("database bytes after reads"),
        database_before
    );
}

#[test]
fn absent_records_are_distinct_from_missing_storage() {
    let fixture = fixture();
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    assert_eq!(
        reader.load_remote_workspace(OWNER, "absent").expect("missing record"),
        None
    );
    assert_eq!(
        reader.load_remote_allocation(OWNER, "absent").expect("missing binding"),
        None
    );
    assert!(
        reader
            .load_remote_workspace("22222222-2222-4222-8222-222222222222", "workspace")
            .is_err()
    );
}

#[test]
fn read_only_open_does_not_migrate_or_repair_storage() {
    for version in [STORE_SCHEMA_VERSION - 1, STORE_SCHEMA_VERSION + 1] {
        let fixture = fixture();
        let connection = fixture.store.connection().expect("writer");
        connection
            .pragma_update(None, "user_version", version)
            .expect("incompatible version");
        drop(connection);
        let before = std::fs::read(fixture.store.path()).expect("database");
        assert!(CloudWorkflowStore::open_read_only_path(fixture.store.path()).is_err());
        assert!(fixture.store.load_remote_workspace(OWNER, "workspace").is_err());
        assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
        assert_eq!(std::fs::read(fixture.store.path()).expect("unchanged database"), before);
    }
    let directory = tempfile::tempdir().expect("private directory");
    let path = directory.path().join("corrupt.sqlite3");
    let bytes = b"not a database";
    std::fs::write(&path, bytes).expect("corrupt fixture");
    assert!(CloudWorkflowStore::open_read_only_path(&path).is_err());
    assert_eq!(std::fs::read(path).expect("unchanged corruption"), bytes);
}

#[cfg(unix)]
#[test]
fn read_only_open_preserves_private_parent_aliases_but_not_database_symlinks() {
    let fixture = fixture();
    let directory = tempfile::tempdir().expect("alias directory");
    let alias = directory.path().join("control");
    std::os::unix::fs::symlink(fixture.store.path().parent().expect("store parent"), &alias).expect("parent alias");
    let path = alias.join("workflows.sqlite3");
    let reader = CloudWorkflowStore::open_read_only_path(&path).expect("private parent alias");
    assert_eq!(reader.path(), fixture.store.path());
    assert_eq!(
        reader.load_remote_workspace(OWNER, "workspace").expect("workspace"),
        Some(fixture.workspace)
    );
    let linked = alias.join("linked.sqlite3");
    std::os::unix::fs::symlink(&path, &linked).expect("database symlink");
    assert!(matches!(
        CloudWorkflowStore::open_read_only_path(linked),
        Err(CloudStoreError::SymlinkStorePath)
    ));
}

#[cfg(unix)]
#[test]
fn read_only_open_rejects_exposed_parent_without_changing_permissions() {
    let fixture = fixture();
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("private reader");
    let path = fixture.store.path();
    let parent = path.parent().expect("store parent");
    let before = std::fs::read(path).expect("database");
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).expect("exposed parent");
    assert!(matches!(
        CloudWorkflowStore::open_read_only_path(path),
        Err(CloudStoreError::InsecureStoreDirectory)
    ));
    assert!(reader.list_remote_environment_page(None).is_err());
    assert!(fixture.store.load_remote_workspace(OWNER, "workspace").is_err());
    assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
    assert_eq!(parent.metadata().expect("parent").permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::read(path).expect("unchanged database"), before);
}

#[cfg(unix)]
#[test]
fn read_only_open_rejects_exposed_database_without_repairing_it() {
    let fixture = fixture();
    let path = fixture.store.path();
    let before = std::fs::read(path).expect("database");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).expect("exposed database");
    assert!(matches!(
        CloudWorkflowStore::open_read_only_path(path),
        Err(CloudStoreError::InsecureStoreFile)
    ));
    assert!(fixture.store.load_remote_workspace(OWNER, "workspace").is_err());
    assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
    assert_eq!(path.metadata().expect("database").permissions().mode() & 0o777, 0o644);
    assert_eq!(std::fs::read(path).expect("unchanged database"), before);
}

#[cfg(unix)]
#[test]
fn read_only_open_and_single_record_reads_reject_symlink_database() {
    let fixture = fixture();
    let path = fixture.store.path();
    let retained = path.with_file_name("retained.sqlite3");
    std::fs::rename(path, &retained).expect("retain database");
    let before = std::fs::read(&retained).expect("retained database");
    std::os::unix::fs::symlink(&retained, path).expect("symlink fixture");
    assert!(matches!(
        CloudWorkflowStore::open_read_only_path(path),
        Err(CloudStoreError::SymlinkStorePath)
    ));
    assert!(fixture.store.load_remote_workspace(OWNER, "workspace").is_err());
    assert!(fixture.store.load_remote_allocation(OWNER, "workspace").is_err());
    assert_eq!(std::fs::read(retained).expect("preserved database"), before);
}
