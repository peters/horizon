use super::*;

#[test]
fn generic_create_cannot_introduce_a_runtime() {
    let (_directory, store) = store();
    let state = provisioning(workspace("workspace"));
    assert!(matches!(
        store.create_remote_workspace(OWNER, &state),
        Err(RemoteWorkspaceStoreError::RuntimeAllocationRequired)
    ));
    assert!(
        store
            .list_remote_workspaces(OWNER)
            .expect("unchanged inventory")
            .is_empty()
    );
}

#[test]
fn generic_replacement_cannot_introduce_a_runtime() {
    let (_directory, store) = store();
    let original = store
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("dormant");
    let next = provisioning(original.state().clone());
    assert!(matches!(
        store.replace_remote_workspace(&original, &next),
        Err(RemoteWorkspaceStoreError::RuntimeAllocationRequired)
    ));
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("retained"),
        Some(original)
    );
}

#[test]
fn non_creating_observations_cannot_revert_to_provisioning() {
    for phase in [RemoteRuntimePhase::Reconciling, RemoteRuntimePhase::Failed] {
        let (_directory, store) = store();
        let mut state = provisioning(workspace("workspace"));
        let original = seed_legacy_workspace(&store, &state);
        state.runtime.as_mut().expect("runtime").phase = phase;
        let observed = store
            .replace_remote_workspace(&original, &state)
            .expect("non-creating observation");
        let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
        state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Provisioning;
        assert!(matches!(
            reopened.replace_remote_workspace(&observed, &state),
            Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
        ));
        assert_eq!(
            reopened.load_remote_workspace(OWNER, "workspace").expect("retained"),
            Some(observed)
        );
    }
}

#[test]
fn every_observed_phase_retains_legacy_identity_and_does_not_return_to_provisioning() {
    for phase in [
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Materializing,
        RemoteRuntimePhase::Ready,
        RemoteRuntimePhase::Checkpointing,
        RemoteRuntimePhase::Cancelling,
        RemoteRuntimePhase::Deleting,
        RemoteRuntimePhase::Failed,
    ] {
        let (_directory, store) = store();
        let mut state = ownership::expired_runtime();
        let runtime = state.runtime.as_mut().expect("runtime");
        runtime.phase = phase;
        if matches!(phase, RemoteRuntimePhase::Cancelling | RemoteRuntimePhase::Deleting) {
            runtime.cleanup = Some(RemoteCleanupIntent {
                reason: RemoteCleanupReason::Cancelled,
                requested_at_millis: 2000,
            });
        }
        let original = seed_legacy_workspace(&store, &state);
        let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
        assert_eq!(
            reopened.list_remote_workspaces(OWNER).expect("recover"),
            vec![original.clone()]
        );
        state.spec.panels.clear();
        let detached = reopened
            .replace_remote_workspace(&original, &state)
            .expect("remove client intent");
        assert_eq!(detached.state().runtime, original.state().runtime);
        state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Provisioning;
        assert!(matches!(
            reopened.replace_remote_workspace(&detached, &state),
            Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
        ));
        assert_eq!(
            reopened.load_remote_workspace(OWNER, "workspace").expect("retained"),
            Some(detached)
        );
    }
}

#[test]
fn the_internal_allocation_path_can_stage_runtime_identity_but_never_commits_itself() {
    let (_directory, store) = store();
    let original = store
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("dormant");
    let next = provisioning(original.state().clone());
    let replacement = WorkspaceReplacement::new(&original, &next).expect("prepare allocation snapshot");
    let mut connection = store.connection().expect("connection");
    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("transaction");
        ensure_current_schema(&transaction).expect("schema");
        let staged = replacement.persist(&transaction).expect("internal allocation staging");
        assert_eq!(staged.state(), &next);
        assert_eq!(staged.revision(), original.revision() + 1);
        // No allocator committed a workflow and binding, so abandon the transaction.
    }
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("rolled back"),
        Some(original)
    );
}
