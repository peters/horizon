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

#[cfg(unix)]
mod existing {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct DatabaseState {
        bytes: Vec<u8>,
        version: i64,
        journal_mode: String,
        rows: SavedRows,
    }

    fn state(fixture: &Fixture) -> DatabaseState {
        let connection = open_read_connection(fixture.store.path()).expect("inspect only");
        DatabaseState {
            bytes: std::fs::read(fixture.store.path()).expect("database bytes"),
            version: connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("version"),
            journal_mode: connection
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .expect("journal"),
            rows: saved_bytes(&connection),
        }
    }

    #[test]
    fn legacy_and_future_schemas_are_refused_without_migration_or_journal_changes() {
        for version in [4, 5, 6, STORE_SCHEMA_VERSION + 1] {
            let fixture = fixture();
            let writer = CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path())
                .expect("current handle before schema drift");
            let connection = open_connection(fixture.store.path()).expect("fixture writer");
            if version < 7 {
                connection
                    .execute_batch("DROP TABLE remote_provider_bindings")
                    .expect("legacy binding");
            }
            if version < 6 {
                connection
                    .execute_batch("DROP TABLE remote_network_volume_selections")
                    .expect("legacy selection");
            }
            if version < 5 {
                connection
                    .execute_batch("DROP TABLE remote_first_pin_intents")
                    .expect("legacy first pin");
            }
            connection
                .pragma_update(None, "user_version", version)
                .expect("schema fixture");
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("non-WAL fixture");
            drop(connection);
            let before = state(&fixture);
            if version < STORE_SCHEMA_VERSION {
                CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("genuine compatible legacy");
            }
            assert!(matches!(
                CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()),
                Err(CloudStoreError::UnsupportedSchema(actual)) if actual == version
            ));
            let mut next = fixture.workflow.workflow().clone();
            next.updated_at_millis += 1;
            assert!(
                matches!(writer.replace(&fixture.workflow, &next), Err(CloudStoreError::UnsupportedSchema(actual)) if actual == version)
            );
            assert_eq!(state(&fixture), before);
        }
    }

    #[test]
    fn base_schema_drift_is_refused_before_existing_open_or_cloned_writes() {
        let changed_constraint = format!(
            "DROP TABLE cloud_workflows; {}",
            SCHEMA.replace("CHECK (revision > 0)", "CHECK (revision >= 0)")
        );
        for sql in [
            "DROP INDEX remote_workspaces_session",
            "DROP INDEX cloud_workflows_retention",
            "DROP INDEX cloud_worker_creation_claims_workflow",
            "DROP TABLE cloud_workflows",
            "DROP TABLE cloud_worker_creation_claims",
            "DROP TABLE remote_workspaces",
            "DROP INDEX cloud_workflows_retention; CREATE INDEX cloud_workflows_retention ON cloud_workflows(updated_at_millis)",
            "ALTER TABLE cloud_workflows RENAME COLUMN snapshot TO altered_snapshot",
            "ALTER TABLE cloud_worker_creation_claims RENAME COLUMN resource_name TO altered_resource_name",
            "ALTER TABLE remote_workspaces RENAME COLUMN snapshot TO altered_snapshot",
            &changed_constraint,
            "CREATE TRIGGER unexpected_base_write AFTER UPDATE ON ClOuD_WoRkFlOwS BEGIN DELETE FROM remote_workspaces; END",
            "CREATE TRIGGER unexpected_base_write AFTER DELETE ON cloud_worker_creation_claims BEGIN DELETE FROM remote_workspaces; END",
            "CREATE TRIGGER unexpected_base_write AFTER UPDATE ON remote_workspaces BEGIN DELETE FROM cloud_workflows; END",
        ] {
            let fixture = fixture();
            let path = fixture.store.path();
            let writer = CloudWorkflowStore::open_existing_without_migration_path(path)
                .expect("current handle before drift")
                .clone();
            let connection = open_connection(path).expect("fixture writer");
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("committed byte comparison");
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .expect("allow incomplete synthetic schema");
            connection.execute_batch(sql).expect("base schema drift");
            drop(connection);
            let inspect = || {
                let connection = open_read_connection(path).expect("inspect without admission");
                let version: i64 = connection
                    .pragma_query_value(None, "user_version", |row| row.get(0))
                    .expect("version");
                let journal: String = connection
                    .pragma_query_value(None, "journal_mode", |row| row.get(0))
                    .expect("journal");
                (std::fs::read(path).expect("all committed bytes"), version, journal)
            };
            let before = inspect();
            assert_eq!(before.1, STORE_SCHEMA_VERSION);
            assert_eq!(before.2, "delete");
            let admitted = CloudWorkflowStore::open_existing_without_migration_path(path);
            assert_eq!(inspect(), before, "opening changed storage: {sql}");
            assert!(
                matches!(admitted, Err(CloudStoreError::InvalidAllocationSchema)),
                "opener admitted schema drift: {sql}: {admitted:?}"
            );
            let mut next = fixture.workflow.workflow().clone();
            next.updated_at_millis += 1;
            assert!(matches!(
                writer.replace(&fixture.workflow, &next),
                Err(CloudStoreError::InvalidAllocationSchema)
            ));
            assert_eq!(inspect(), before, "cloned writer changed storage: {sql}");
        }
    }

    #[test]
    fn current_schema_open_preserves_storage_and_cloned_writers_use_exact_cas() {
        for journal in ["delete", "wal"] {
            let fixture = fixture();
            let connection = open_connection(fixture.store.path()).expect("fixture writer");
            connection
                .pragma_update(None, "journal_mode", journal)
                .expect("journal fixture");
            connection
                .execute(
                    "CREATE INDEX unrelated_workflow_index ON cloud_workflows(updated_at_millis)",
                    [],
                )
                .expect("unrelated index remains supported");
            drop(connection);
            let before = state(&fixture);
            let writer = CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path())
                .expect("current existing writer");
            let writer = writer.clone();
            assert_eq!(
                writer.load(fixture.workflow.workflow().id).expect("read"),
                Some(fixture.workflow.clone())
            );
            assert_eq!(state(&fixture), before);
            let mut next = fixture.workflow.workflow().clone();
            next.updated_at_millis += 1;
            let updated = writer.replace(&fixture.workflow, &next).expect("verified exact CAS");
            assert_eq!(updated.revision(), fixture.workflow.revision() + 1);
            assert_eq!(updated.workflow(), &next);
            assert_eq!(writer.load(next.id).expect("saved CAS"), Some(updated));
            assert!(matches!(
                writer.replace(&fixture.workflow, &next),
                Err(CloudStoreError::RevisionConflict { .. })
            ));
            let after = state(&fixture);
            assert_eq!(after.version, before.version);
            assert_eq!(after.journal_mode, journal);
            assert_eq!(after.rows.workspace, before.rows.workspace);
            assert_eq!(after.rows.claims, before.rows.claims);
        }
    }

    #[test]
    fn missing_corrupt_and_nonregular_paths_are_not_created_or_repaired() {
        let directory = tempfile::tempdir().expect("private directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private fixture parent");
        let home = crate::HorizonHome::from_root(directory.path().join("absent"));
        assert!(CloudWorkflowStore::open_existing_without_migration(&home).is_err());
        assert!(!home.root().exists());
        let path = directory.path().join("database");
        assert!(CloudWorkflowStore::open_existing_without_migration_path(&path).is_err());
        assert!(!path.exists());
        std::fs::create_dir(&path).expect("nonregular fixture");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("private directory");
        assert!(matches!(
            CloudWorkflowStore::open_existing_without_migration_path(&path),
            Err(CloudStoreError::ExistingStoreChanged)
        ));
        assert!(path.is_dir());
        let empty = directory.path().join("empty.sqlite3");
        std::fs::write(&empty, b"").expect("empty fixture");
        std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o600)).expect("private empty fixture");
        assert!(matches!(
            CloudWorkflowStore::open_existing_without_migration_path(&empty),
            Err(CloudStoreError::UnsupportedSchema(0))
        ));
        assert_eq!(std::fs::metadata(empty).expect("unchanged empty file").len(), 0);
        let corrupt = directory.path().join("corrupt.sqlite3");
        std::fs::write(&corrupt, b"not a database").expect("corrupt fixture");
        std::fs::set_permissions(&corrupt, std::fs::Permissions::from_mode(0o600)).expect("private corruption");
        assert!(CloudWorkflowStore::open_existing_without_migration_path(&corrupt).is_err());
        assert_eq!(std::fs::read(corrupt).expect("preserved corruption"), b"not a database");
        let fixture = fixture();
        let connection = open_connection(fixture.store.path()).expect("fixture writer");
        connection
            .execute_batch("DROP INDEX remote_runtime_allocations_job")
            .expect("schema corruption");
        drop(connection);
        let before = state(&fixture);
        assert!(matches!(
            CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
        assert_eq!(state(&fixture), before);
    }

    #[test]
    fn existing_clones_reject_disappearance_replacement_and_symlinks_without_recreation() {
        for replacement in ["absent", "copy", "symlink"] {
            let fixture = fixture();
            let path = fixture.store.path();
            let writer = CloudWorkflowStore::open_existing_without_migration_path(path).expect("existing writer");
            let writer = writer.clone();
            let retained = path.with_file_name("retained.sqlite3");
            std::fs::rename(path, &retained).expect("retain original");
            let before = std::fs::read(&retained).expect("retained bytes");
            match replacement {
                "copy" => {
                    std::fs::copy(&retained, path).expect("same-byte replacement");
                }
                "symlink" => std::os::unix::fs::symlink(&retained, path).expect("symlink replacement"),
                _ => {}
            }
            let mut next = fixture.workflow.workflow().clone();
            next.updated_at_millis += 1;
            assert!(writer.replace(&fixture.workflow, &next).is_err());
            assert_eq!(std::fs::read(&retained).expect("unchanged original"), before);
            if replacement == "absent" {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(path).expect("unchanged replacement"), before);
            }
        }
    }

    #[test]
    fn exposed_file_or_parent_is_refused_without_permission_repair() {
        for parent in [false, true] {
            let fixture = fixture();
            let writer =
                CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()).expect("writer");
            let before = std::fs::read(fixture.store.path()).expect("original");
            let changed = if parent {
                fixture.store.path().parent().expect("parent")
            } else {
                fixture.store.path()
            };
            let mode = if parent { 0o755 } else { 0o644 };
            std::fs::set_permissions(changed, std::fs::Permissions::from_mode(mode)).expect("exposed fixture");
            assert!(CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()).is_err());
            assert!(writer.connection().is_err());
            assert_eq!(changed.metadata().expect("metadata").permissions().mode() & 0o777, mode);
            assert_eq!(std::fs::read(fixture.store.path()).expect("unchanged bytes"), before);
        }
    }
}

#[cfg(not(unix))]
#[test]
fn existing_writer_is_explicitly_unsupported_without_creating_storage() {
    let directory = tempfile::tempdir().expect("private directory");
    let home = crate::HorizonHome::from_root(directory.path().join("absent"));
    assert!(matches!(
        CloudWorkflowStore::open_existing_without_migration(&home),
        Err(CloudStoreError::ExistingStoreUnsupported)
    ));
    assert!(!home.root().exists());
}
