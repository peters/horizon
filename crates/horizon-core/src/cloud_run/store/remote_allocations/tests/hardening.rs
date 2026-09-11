use super::super::super::{MAX_RECOVERED_SNAPSHOT_BYTES, MAX_SNAPSHOT_BYTES};
use super::first_pin::{intent_count, reserved, status};
use super::*;
use crate::PanelKind;
use crate::remote_workspace::RemotePanelBinding;

#[test]
fn first_pin_schema_four_inventory_and_clones_remain_noncreating() {
    let (fixture, saved) = reserved();
    assert!(claim(&fixture.store, &saved).expect("legacy claim"));
    let connection = Connection::open(fixture.store.path()).expect("fixture connection");
    connection
        .execute_batch(
            "DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; PRAGMA user_version=4",
        )
        .expect("legacy schema");
    let before: (Vec<u8>, Vec<u8>, i64) = connection
        .query_row(
            "SELECT (SELECT snapshot FROM remote_workspaces), (SELECT snapshot FROM cloud_workflows),
         (SELECT claimed_at_millis FROM cloud_worker_creation_claims)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("legacy records");
    let read_only = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("legacy inventory opener");
    for reader in [read_only.clone(), read_only] {
        let page = reader.list_remote_environment_page(None).expect("zero-view inventory");
        assert_eq!(page.records, vec![saved.workspace().clone()]);
        assert_eq!(page.next_cursor, None);
        assert_eq!(
            reader.load_remote_workspace(OWNER, "workspace").expect("workspace"),
            Some(saved.workspace().clone())
        );
        assert_eq!(
            reader.load_remote_allocation(OWNER, "workspace").expect("allocation"),
            Some(saved.clone())
        );
        assert_eq!(
            reader.load(saved.workflow().workflow().id).expect("workflow"),
            Some(saved.workflow().clone())
        );
        assert_eq!(
            reader
                .load_remote_first_pin_request(&saved)
                .expect("legacy is not intent"),
            None
        );
        assert!(matches!(
            reader.create(&retained_workflow(CloudProvider::LocalDocker, 1000)),
            Err(CloudStoreError::Database(_))
        ));
    }
    assert!(matches!(
        fixture.store.record_remote_first_pin_intent(&saved),
        Err(Error::Storage(CloudStoreError::UnsupportedSchema(4)))
    ));
    let after = connection
        .query_row(
            "SELECT (SELECT snapshot FROM remote_workspaces), (SELECT snapshot FROM cloud_workflows),
         (SELECT claimed_at_millis FROM cloud_worker_creation_claims)",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("unchanged records");
    assert_eq!(after, before);
    assert_eq!(fixture.counts(), [1, 1, 1]);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("not migrated"),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='remote_first_pin_intents'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .expect("no backfill"),
        0
    );
}

#[test]
fn first_pin_schema_four_refuses_partial_objects_and_query_only_writers() {
    for corruption in [
        "DROP TABLE remote_network_volume_selections",
        "DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; DROP INDEX remote_runtime_allocations_workflow",
        "DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; DROP TABLE remote_runtime_creation_fences",
    ] {
        let (fixture, saved) = reserved();
        let connection = Connection::open(fixture.store.path()).expect("fixture connection");
        connection.execute_batch(corruption).expect("partial fixture");
        connection
            .pragma_update(None, "user_version", 4)
            .expect("legacy version");
        assert!(CloudWorkflowStore::open_read_only_path(fixture.store.path()).is_err());
        assert!(fixture.store.load_remote_first_pin_request(&saved).is_err());
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .expect("unchanged version"),
            4
        );
    }
    let (fixture, _) = reserved();
    let connection = Connection::open(fixture.store.path()).expect("fixture connection");
    connection
        .execute_batch("DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; PRAGMA user_version=4; PRAGMA query_only=ON")
        .expect("query-only writer");
    assert!(
        !connection
            .is_readonly(rusqlite::MAIN_DB)
            .expect("actual connection mode")
    );
    assert!(matches!(
        ensure_current_schema(&connection),
        Err(CloudStoreError::UnsupportedSchema(4))
    ));
}

#[test]
fn first_pin_intent_rejects_other_providers_management_and_saved_ssh_without_worker() {
    let fixture = fixture();
    let allocation = fixture.allocate();
    let saved = fixture
        .store
        .reserve_remote_worker_request(
            &allocation,
            &observed_worker(allocation.workspace().state()).ssh_public_key,
        )
        .expect("local request");
    assert!(matches!(
        fixture.store.record_remote_first_pin_intent(&saved),
        Err(Error::RuntimeSetupUnavailable)
    ));
    assert_eq!(intent_count(&fixture.store), 0);
    let (fixture, saved) = reserved();
    fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
    let mut state = saved.workspace().state().clone();
    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1000,
    });
    fixture
        .store
        .replace_remote_workspace(saved.workspace(), &state)
        .expect("cleanup");
    let current = fixture.recover();
    assert!(matches!(
        fixture.store.record_remote_first_pin_intent(&current),
        Err(Error::RuntimeSetupUnavailable)
    ));
    assert!(matches!(
        fixture.store.load_remote_first_pin_request(&current),
        Err(Error::RuntimeRecoveryUnavailable)
    ));
    assert_eq!(intent_count(&fixture.store), 1);
    let (fixture, saved) = reserved();
    let connection = Connection::open(fixture.store.path()).expect("fixture connection");
    let bytes: Vec<u8> = connection
        .query_row("SELECT snapshot FROM remote_workspaces", [], |row| row.get(0))
        .expect("snapshot");
    let mut snapshot: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    snapshot["state"]["runtime"]["ssh"] = serde_json::to_value(status(&saved, true).ssh).expect("endpoint");
    connection
        .execute(
            "UPDATE remote_workspaces SET snapshot=?1",
            [serde_json::to_vec(&snapshot).expect("corrupt JSON")],
        )
        .expect("corrupt fixture");
    assert!(matches!(
        fixture.store.record_remote_first_pin_intent(&saved),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    assert_eq!(intent_count(&fixture.store), 0);
}

#[test]
fn first_pin_lookup_refuses_schema_drift_and_missing_storage_without_repair() {
    for corruption in [
        "DROP TABLE remote_first_pin_intents",
        "CREATE INDEX unexpected_first_pin_index ON remote_first_pin_intents(generation)",
        "ALTER TABLE remote_first_pin_intents ADD COLUMN unexpected INTEGER",
        "CREATE TRIGGER unexpected_first_pin_trigger AFTER DELETE ON remote_first_pin_intents BEGIN SELECT 1; END",
    ] {
        let (fixture, saved) = reserved();
        fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
        Connection::open(fixture.store.path())
            .expect("fixture connection")
            .execute_batch(corruption)
            .expect("schema drift");
        assert!(matches!(
            fixture.store.load_remote_first_pin_request(&saved),
            Err(Error::Storage(CloudStoreError::InvalidAllocationSchema))
        ));
        assert!(matches!(
            CloudWorkflowStore::open_path(fixture.store.path()),
            Err(CloudStoreError::InvalidAllocationSchema)
        ));
    }
    let (fixture, saved) = reserved();
    fixture.store.record_remote_first_pin_intent(&saved).expect("intent");
    let parent = fixture.store.path().parent().expect("private parent");
    let retained = parent.with_file_name("retained-control");
    std::fs::rename(parent, &retained).expect("retain disappeared fixture database");
    assert!(fixture.store.load_remote_first_pin_request(&saved).is_err());
    assert!(!parent.exists());
    assert!(!fixture.store.path().exists());
    std::fs::rename(retained, parent).expect("restore fixture");
    assert_eq!(fixture.recover(), saved);
    assert_eq!(intent_count(&fixture.store), 1);
}

#[test]
fn invalid_setup_retention_and_exhausted_generation_never_allocate() {
    let fixture = fixture();
    let store = &fixture.store;
    for until in [-1, 0, 1000, current_unix_millis().expect("now")] {
        assert!(matches!(
            store.allocate_remote_runtime(&fixture.original, until),
            Err(Error::InvalidAllocationRetention)
        ));
    }
    let mut state = fixture.original.state().clone();
    state.spec.workspace_local_id = "exhausted".into();
    state.spec.generation = u64::MAX;
    let exhausted = store.create_remote_workspace(OWNER, &state).expect("counter record");
    assert!(matches!(
        store.allocate_remote_runtime(&exhausted, i64::MAX),
        Err(Error::GenerationExhausted)
    ));
    assert_eq!(fixture.counts(), [0, 0, 0]);
}

#[test]
fn zero_panels_and_detached_intents_do_not_control_lifetime_or_renew_creation() {
    for lifetime in [WorkerLifetime::Persistent, WorkerLifetime::TimeLimited { seconds: 900 }] {
        let fixture = fixture();
        let store = &fixture.store;
        let mut state = fixture.original.state().clone();
        state.spec.target.lifetime = lifetime;
        state.spec.panels.clear();
        let empty = store
            .replace_remote_workspace(&fixture.original, &state)
            .expect("empty intent");
        let saved = store
            .allocate_remote_runtime(&empty, i64::MAX)
            .expect("allocate without panels");
        assert_eq!(saved.workspace().state().spec.target.lifetime, lifetime);
        assert!(
            saved
                .workspace()
                .state()
                .runtime
                .as_ref()
                .expect("runtime")
                .cleanup
                .is_none()
        );
        assert!(claim(store, &saved).expect("first grant"));
        let mut attached = saved.workspace().state().clone();
        attached.spec.panels.push(RemotePanelBinding {
            panel_local_id: "panel".into(),
            kind: PanelKind::Shell,
            command: None,
            working_directory: None,
            task_handoff: None,
            agent_session_id: None,
        });
        let with_panel = store
            .replace_remote_workspace(saved.workspace(), &attached)
            .expect("attach intent");
        assert_eq!(with_panel.state().spec.panels.len(), 1);
        attached.spec.panels.clear();
        let detached = store
            .replace_remote_workspace(&with_panel, &attached)
            .expect("detach intent");
        assert_eq!(detached.state().runtime, saved.workspace().state().runtime);
        let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
        assert!(!claim(&reopened, &saved).expect("no second grant"));
        assert!(matches!(
            reopened.allocate_remote_runtime(&detached, i64::MAX),
            Err(Error::RuntimeAlreadyActive)
        ));
        assert_eq!(fixture.counts(), [1, 1, 1]);
    }
}

#[test]
fn broken_cross_record_identity_never_grants_creation() {
    #[derive(Clone, Copy)]
    enum Missing {
        Binding,
        Workflow,
        Neither,
    }
    for (missing, sql) in [
        (Missing::Binding, "DELETE FROM remote_runtime_allocations"),
        (Missing::Workflow, "DELETE FROM cloud_workflows"),
        (Missing::Neither, "DELETE FROM remote_workspaces"),
        (
            Missing::Neither,
            "UPDATE remote_runtime_allocations SET session_id='22222222-2222-4222-8222-222222222222'",
        ),
        (Missing::Neither, "UPDATE remote_runtime_allocations SET generation=2"),
        (
            Missing::Neither,
            "UPDATE cloud_workflows SET snapshot=CAST(json_set(CAST(snapshot AS TEXT), '$.nodes[0].source.repository', 'other/project') AS BLOB)",
        ),
    ] {
        let fixture = fixture();
        let store = &fixture.store;
        let saved = fixture.allocate();
        let connection = Connection::open(store.path()).expect("raw store");
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("synthetic corruption");
        connection.execute_batch(sql).expect("corruption");
        let recovered = store.load_remote_allocation(OWNER, "workspace");
        if matches!(missing, Missing::Binding) {
            assert!(matches!(recovered, Err(Error::UnboundRuntime)), "{sql}");
        } else {
            assert!(
                matches!(recovered, Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))),
                "{sql}"
            );
        }
        let granted = claim(store, &saved);
        if matches!(missing, Missing::Workflow) {
            assert!(matches!(granted, Err(CloudStoreError::WorkflowMissing(_))), "{sql}");
        } else {
            assert!(
                matches!(granted, Err(CloudStoreError::InvalidRemoteAllocation)),
                "{sql}"
            );
        }
        assert_eq!(fixture.counts()[2], 0);
    }
}

#[test]
fn active_legacy_unbound_records_stay_recoverable_without_allocation() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut state = fixture.original.state().clone();
    state.spec.workspace_local_id = "legacy".into();
    let PreparedAllocation {
        next: state,
        mut workflow,
    } = prepare_allocation(&state, 1000, i64::MAX).expect("legacy fixture");
    workflow.nodes[0].kind = WorkflowNodeKind::Build;
    store.create(&workflow).expect("ordinary unclaimed legacy workflow");
    state.validate().expect("valid legacy aggregate");
    let snapshot =
        serde_json::to_vec(&serde_json::json!({"session_id": OWNER, "state": state})).expect("legacy snapshot");
    let connection = Connection::open(store.path()).expect("raw legacy fixture");
    connection
        .execute_batch(
            "DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; DROP TABLE remote_runtime_creation_fences; PRAGMA user_version=3",
        )
        .expect("schema-three fixture before migration");
    connection
        .execute(
            "INSERT INTO remote_workspaces VALUES (?1, ?2, 7, ?3)",
            params!["legacy", OWNER, snapshot],
        )
        .expect("already-persisted legacy record");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("migrate legacy runtime");
    let legacy = reopened
        .load_remote_workspace(OWNER, "legacy")
        .expect("legacy recovery")
        .expect("legacy record");
    assert_eq!(legacy.state(), &state);
    assert_eq!(legacy.revision(), 7);
    assert_eq!(
        connection
            .query_row(
                "SELECT snapshot FROM remote_workspaces WHERE workspace_local_id='legacy'",
                [],
                |row| { row.get::<_, Vec<u8>>(0) }
            )
            .expect("unchanged snapshot bytes"),
        snapshot
    );
    assert!(matches!(
        reopened.load_remote_allocation(OWNER, "legacy"),
        Err(Error::UnboundRuntime)
    ));
    assert!(matches!(
        reopened.allocate_remote_runtime(&legacy, i64::MAX),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert!(matches!(
        reopened.claim_worker_creation(workflow.id, workflow.nodes[0].id, &state.spec.target, "legacy-worker"),
        Err(CloudStoreError::LegacyRuntimeCreationDenied)
    ));
    assert_eq!(creation_denial_count(&connection), 1);
    assert_eq!(
        reopened
            .load_remote_workspace(OWNER, "legacy")
            .expect("record recovery"),
        Some(legacy)
    );
    assert_eq!(fixture.counts(), [1, 0, 0]);
}

#[test]
fn migrated_bound_allocations_still_require_their_positive_binding() {
    for lost_binding in [false, true] {
        let fixture = fixture();
        let saved = fixture.allocate();
        let connection = Connection::open(fixture.store.path()).expect("raw migration fixture");
        connection
            .execute_batch(
                "DROP TABLE remote_network_volume_selections; DROP TABLE remote_first_pin_intents; DROP TABLE remote_runtime_creation_fences; PRAGMA user_version=3",
            )
            .expect("schema-three bound allocation");
        let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("migrate bound allocation");
        assert_eq!(creation_denial_count(&connection), 1);
        assert_eq!(
            reopened.load_remote_allocation(OWNER, "workspace").expect("recover"),
            Some(saved.clone())
        );
        if lost_binding {
            connection
                .execute("DELETE FROM remote_runtime_allocations", [])
                .expect("lose migrated binding");
            assert!(matches!(
                claim(&reopened, &saved),
                Err(CloudStoreError::InvalidRemoteAllocation)
            ));
        } else {
            assert!(claim(&reopened, &saved).expect("one positive grant"));
            assert!(!claim(&reopened, &saved).expect("no duplicate grant"));
        }
        assert_eq!(creation_denial_count(&connection), 1);
        assert_eq!(
            fixture.counts(),
            [1, i64::from(!lost_binding), i64::from(!lost_binding)]
        );
    }
}

fn creation_denial_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM remote_runtime_creation_fences", [], |row| {
            row.get(0)
        })
        .expect("creation denials")
}

#[test]
fn ordinary_workflows_cannot_copy_allocated_jobs_even_after_binding_loss() {
    for lost_binding in [false, true] {
        for through_replacement in [false, true] {
            let fixture = fixture();
            let store = &fixture.store;
            let saved = fixture.allocate();
            let job_id = saved.workflow().workflow().nodes[0].id;
            let mut copied = saved.workflow().workflow().clone();
            copied.id = CloudWorkflowId::new();
            copied.nodes[0].kind = WorkflowNodeKind::Build;
            if through_replacement {
                copied.nodes[0].id = CloudJobId::new();
            }
            let ordinary = store.create(&copied).expect("ordinary workflow");
            if through_replacement {
                copied.nodes[0].id = job_id;
                copied.updated_at_millis += 1;
                store.replace(&ordinary, &copied).expect("ordinary replacement");
            }
            if lost_binding {
                Connection::open(store.path())
                    .expect("raw store")
                    .execute("DELETE FROM remote_runtime_allocations", [])
                    .expect("lost allocation binding");
                assert!(matches!(
                    claim(store, &saved),
                    Err(CloudStoreError::InvalidRemoteAllocation)
                ));
            } else {
                assert!(claim(store, &saved).expect("original grant"));
            }
            assert!(matches!(
                store.claim_worker_creation(
                    copied.id,
                    job_id,
                    &saved.workspace().state().spec.target,
                    "copied-worker"
                ),
                Err(CloudStoreError::LegacyRuntimeCreationDenied)
            ));
            assert_eq!(
                fixture.counts(),
                [2, i64::from(!lost_binding), i64::from(!lost_binding)]
            );
        }
    }
}

#[test]
fn separate_workspaces_keep_distinct_workflow_and_job_identities() {
    let fixture = fixture();
    let store = &fixture.store;
    let first = fixture.allocate();
    let mut state = fixture.original.state().clone();
    state.spec.workspace_local_id = "second".into();
    let second = store.create_remote_workspace(OWNER, &state).expect("second record");
    let second = store
        .allocate_remote_runtime(&second, i64::MAX)
        .expect("independent allocation");
    assert_ne!(first.workflow().workflow().id, second.workflow().workflow().id);
    assert_ne!(
        first.workflow().workflow().nodes[0].id,
        second.workflow().workflow().nodes[0].id
    );
    assert_eq!(fixture.recover(), first);
    assert_eq!(
        store.load_remote_allocation(OWNER, "second").expect("second recovery"),
        Some(second)
    );
    assert_eq!(fixture.counts(), [2, 2, 0]);
}

#[test]
fn same_revision_snapshot_drift_rejects_allocation_without_overwriting_the_record() {
    let fixture = fixture();
    let store = &fixture.store;
    Connection::open(store.path())
        .expect("raw store")
        .execute_batch(
            "UPDATE remote_workspaces SET snapshot=CAST(json_set(CAST(snapshot AS TEXT),
         '$.state.spec.working_directory', 'other') AS BLOB)",
        )
        .expect("same-revision drift");
    assert!(matches!(
        store.allocate_remote_runtime(&fixture.original, i64::MAX),
        Err(Error::SnapshotConflict)
    ));
    let current = store
        .load_remote_workspace(OWNER, "workspace")
        .expect("record")
        .expect("present");
    assert_eq!(current.revision(), fixture.original.revision());
    assert_eq!(current.state().spec.working_directory, "other");
    assert_eq!(fixture.counts(), [0, 0, 0]);
}

#[test]
fn a_full_session_byte_budget_rejects_runtime_growth_without_partial_writes() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut connection = store.connection().expect("store connection");
    let original_bytes: i64 = connection
        .query_row(
            "SELECT length(snapshot) FROM remote_workspaces WHERE workspace_local_id='workspace'",
            [],
            |row| row.get(0),
        )
        .expect("original size");
    let original_bytes = usize::try_from(original_bytes).expect("bounded original size");
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("fixture transaction");
    ensure_current_schema(&transaction).expect("current schema");
    let full_rows = MAX_RECOVERED_SNAPSHOT_BYTES / MAX_SNAPSHOT_BYTES;
    for index in 0..full_rows {
        let mut state = fixture.original.state().clone();
        state.spec.workspace_local_id = format!("filled-{index}");
        let mut snapshot =
            serde_json::to_vec(&serde_json::json!({"session_id": OWNER, "state": state})).expect("valid snapshot");
        snapshot.resize(
            MAX_SNAPSHOT_BYTES - if index + 1 == full_rows { original_bytes } else { 0 },
            b' ',
        );
        transaction
            .execute(
                "INSERT INTO remote_workspaces VALUES (?1, ?2, 1, ?3)",
                params![state.spec.workspace_local_id, OWNER, snapshot],
            )
            .expect("capacity fixture");
    }
    transaction.commit().expect("full session");
    assert_eq!(
        store
            .list_remote_workspaces(OWNER)
            .expect("recoverable full session")
            .len(),
        full_rows + 1
    );
    assert!(matches!(
        store.allocate_remote_runtime(&fixture.original, i64::MAX),
        Err(Error::RecoveryLimitExceeded)
    ));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(fixture.original.clone())
    );
    assert_eq!(fixture.counts(), [0, 0, 0]);
}

#[test]
fn a_full_retained_workflow_byte_budget_rolls_back_the_pending_runtime() {
    let fixture = fixture();
    let store = &fixture.store;
    let mut connection = store.connection().expect("store connection");
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("fixture transaction");
    ensure_current_schema(&transaction).expect("current schema");
    let full_rows = MAX_RECOVERED_SNAPSHOT_BYTES / MAX_SNAPSHOT_BYTES;
    for _ in 0..full_rows {
        let workflow = retained_workflow(CloudProvider::LocalDocker, 1000);
        let mut snapshot = encode_workflow(&workflow).expect("valid workflow");
        snapshot.resize(MAX_SNAPSHOT_BYTES, b' ');
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
            .expect("capacity fixture");
    }
    transaction.commit().expect("full workflow budget");
    assert_eq!(
        store
            .list_retained(current_unix_millis().expect("clock"))
            .expect("recoverable full set")
            .len(),
        full_rows
    );
    assert!(matches!(
        store.allocate_remote_runtime(&fixture.original, i64::MAX),
        Err(Error::Storage(CloudStoreError::RecoveryLimitExceeded))
    ));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(fixture.original.clone())
    );
    assert_eq!(
        fixture.counts(),
        [i64::try_from(full_rows).expect("bounded count"), 0, 0]
    );
    assert_eq!(creation_denial_count(&connection), 0);
}
