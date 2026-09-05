use super::*;

#[test]
fn allocation_and_recovery_bind_one_generation_to_one_explicit_worker_node() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    let saved = allocate(&store, &original);
    let state = saved.workspace().state();
    let runtime = state.runtime.as_ref().expect("runtime");
    let workflow = saved.workflow().workflow();
    assert_eq!(saved.workspace().revision(), 2);
    assert_eq!(state.spec.generation, 1);
    assert_eq!(runtime.workflow_id, workflow.id);
    assert_eq!(workflow.nodes.len(), 1);
    assert_eq!(runtime.job_id, workflow.nodes[0].id);
    assert_eq!(workflow.nodes[0].kind, WorkflowNodeKind::RemoteWorkspace);
    assert_eq!(workflow.nodes[0].worker.as_ref(), Some(&state.spec.target));
    assert_eq!(workflow.nodes[0].source.as_ref(), Some(&state.spec.repository));
    assert_eq!(counts(&store), (1, 1, 0));
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_allocation(OWNER, "workspace").expect("recover"),
        Some(saved.clone())
    );
    assert_eq!(
        reopened.load(workflow.id).expect("workflow"),
        Some(saved.workflow().clone())
    );
    assert!(matches!(
        reopened.allocate_remote_runtime(&original, 1000, 901_000),
        Err(Error::RevisionConflict { .. })
    ));
    assert!(matches!(
        reopened.allocate_remote_runtime(saved.workspace(), 1000, 901_000),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert_eq!(counts(&store), (1, 1, 0));
    assert!(matches!(
        reopened.load_remote_allocation(OTHER_OWNER, "workspace"),
        Err(Error::OwnershipMismatch)
    ));
}

#[test]
fn competing_allocators_commit_exactly_one_complete_relationship() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    let barrier = Arc::new(Barrier::new(3));
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let original = original.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.allocate_remote_runtime(&original, 1000, 901_000)
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
            .filter(|result| matches!(result, Err(Error::RevisionConflict { expected: 1, actual: 2 })))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).expect("winner");
    assert_eq!(
        store.load_remote_allocation(OWNER, "workspace").expect("recover"),
        Some(winner)
    );
    assert_eq!(counts(&store), (1, 1, 0));
}

#[test]
fn failure_after_both_snapshots_are_written_rolls_back_every_record() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_binding BEFORE INSERT ON remote_runtime_allocations BEGIN
            SELECT RAISE(ABORT, 'synthetic binding failure'); END;",
        )
        .expect("failure injection");
    assert!(store.allocate_remote_runtime(&original, 1000, 901_000).is_err());
    assert_eq!(counts(&store), (0, 0, 0));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("unchanged"),
        Some(original.clone())
    );
    connection
        .execute_batch("DROP TRIGGER fail_binding")
        .expect("remove injection");
    let saved = allocate(&store, &original);
    assert_eq!(saved.workspace().state().spec.generation, 1);
}

#[test]
fn active_unbound_records_are_recoverable_but_never_receive_an_allocation() {
    let (_directory, store) = store();
    let state = provisioning(workspace("workspace"));
    let original = store.create_remote_workspace(OWNER, &state).expect("unbound record");
    assert!(matches!(
        store.load_remote_allocation(OWNER, "workspace"),
        Err(Error::UnboundRuntime)
    ));
    assert!(matches!(
        store.allocate_remote_runtime(&original, 1000, 901_000),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert_eq!(
        store
            .load_remote_workspace(OWNER, "workspace")
            .expect("recovery record"),
        Some(original)
    );
    assert_eq!(counts(&store), (0, 0, 0));
    assert!(
        store
            .load_remote_allocation(OWNER, "missing")
            .expect("missing")
            .is_none()
    );
    dormant(&store, "dormant");
    assert!(
        store
            .load_remote_allocation(OWNER, "dormant")
            .expect("unallocated")
            .is_none()
    );
}

#[test]
fn invalid_retention_and_exhausted_counters_leave_no_allocation() {
    let (_directory, store) = store();
    let original = dormant(&store, "workspace");
    for (now, until) in [(-1, 901_000), (1000, 999), (1000, 1000), (i64::MAX, i64::MAX)] {
        assert!(matches!(
            store.allocate_remote_runtime(&original, now, until),
            Err(Error::InvalidAllocationRetention)
        ));
    }
    let mut state = workspace("exhausted");
    state.spec.generation = u64::MAX;
    let exhausted = store.create_remote_workspace(OWNER, &state).expect("counter record");
    assert!(matches!(
        store.allocate_remote_runtime(&exhausted, 1000, 901_000),
        Err(Error::GenerationExhausted)
    ));
    let connection = Connection::open(store.path()).expect("raw store");
    connection
        .execute(
            "UPDATE remote_workspaces SET revision = ?1 WHERE workspace_local_id = 'workspace'",
            [i64::MAX],
        )
        .expect("exhausted revision");
    let exhausted = store
        .load_remote_workspace(OWNER, "workspace")
        .expect("reload")
        .expect("record");
    assert!(matches!(
        store.allocate_remote_runtime(&exhausted, 1000, 901_000),
        Err(Error::RevisionExhausted)
    ));
    assert_eq!(counts(&store), (0, 0, 0));
}
