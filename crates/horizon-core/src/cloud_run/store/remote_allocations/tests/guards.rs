use super::*;

#[test]
fn legacy_cleanup_records_remain_visible_without_replacement_creation() {
    for reason in [
        RemoteCleanupReason::LastPanelClosed,
        RemoteCleanupReason::WorkspaceRemoved,
        RemoteCleanupReason::ApplicationExit,
        RemoteCleanupReason::Cancelled,
        RemoteCleanupReason::Failed,
        RemoteCleanupReason::LeaseExpired,
    ] {
        let (_directory, store) = store();
        let saved = allocate(&store, &dormant(&store, "workspace"));
        let mut next = saved.workspace().state().clone();
        next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
            reason,
            requested_at_millis: 2000,
        });
        let retained = store
            .replace_remote_workspace(saved.workspace(), &next)
            .expect("legacy record");
        let workflow = saved.workflow().workflow();
        let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
        let recovered = reopened
            .load_remote_allocation(OWNER, "workspace")
            .expect("recover")
            .expect("allocation");
        assert_eq!(recovered.workspace(), &retained);
        assert!(matches!(
            reopened.claim_worker_creation(workflow.id, workflow.nodes[0].id, &next.spec.target, "synthetic-worker"),
            Err(CloudStoreError::ClaimTargetNotReady(_))
        ));
        assert_eq!(counts(&reopened), (1, 1, 0));
    }
}

#[test]
fn allocation_creation_claim_is_durable_and_cleanup_disables_new_grants() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    let workflow = saved.workflow().workflow();
    let node = &workflow.nodes[0];
    let target = node.worker.as_ref().expect("target");
    assert!(
        store
            .claim_worker_creation(workflow.id, node.id, target, "synthetic-worker")
            .expect("first grant")
    );
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert!(
        !reopened
            .claim_worker_creation(workflow.id, node.id, target, "synthetic-worker")
            .expect("no second grant")
    );
    assert_eq!(counts(&store), (1, 1, 1));
    let mut next = saved.workspace().state().clone();
    let runtime = next.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 2000,
    });
    store
        .replace_remote_workspace(saved.workspace(), &next)
        .expect("cleanup intent");
    assert!(matches!(
        store.claim_worker_creation(workflow.id, node.id, target, "synthetic-worker"),
        Err(CloudStoreError::ClaimTargetNotReady(_))
    ));
    assert!(
        store
            .load_remote_allocation(OWNER, "workspace")
            .expect("cleanup recovery")
            .is_some()
    );
}

#[test]
fn generic_record_writes_cannot_clear_or_adopt_a_bound_runtime() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    let mut cleared = saved.workspace().state().clone();
    cleared.runtime = None;
    assert!(matches!(
        store.replace_remote_workspace(saved.workspace(), &cleared),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    let mut copy = saved.workspace().state().clone();
    copy.spec.workspace_local_id = "copy".into();
    copy.runtime.as_mut().expect("runtime").workspace_local_id = "copy".into();
    assert!(matches!(
        store.create_remote_workspace(OTHER_OWNER, &copy),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    let copy_record = store
        .create_remote_workspace(OTHER_OWNER, &workspace("copy"))
        .expect("dormant copy");
    assert!(matches!(
        store.replace_remote_workspace(&copy_record, &copy),
        Err(Error::Storage(CloudStoreError::InvalidRemoteAllocation))
    ));
    assert_eq!(counts(&store), (1, 1, 0));
    assert_eq!(
        store.load_remote_allocation(OWNER, "workspace").expect("unchanged"),
        Some(saved)
    );
}

#[test]
fn generic_workflow_writes_cannot_create_or_detach_a_remote_allocation() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    let workflow = saved.workflow().workflow();
    let mut copy = workflow.clone();
    copy.id = CloudWorkflowId::new();
    assert!(matches!(
        store.create(&copy),
        Err(CloudStoreError::RemoteAllocationRequired)
    ));
    let mut changed = workflow.clone();
    changed.updated_at_millis += 1;
    changed.nodes[0].kind = WorkflowNodeKind::Build;
    assert!(matches!(
        store.replace(saved.workflow(), &changed),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    changed = workflow.clone();
    changed.updated_at_millis += 1;
    changed.nodes[0].worker.as_mut().expect("target").disk_gib += 1;
    assert!(matches!(
        store.replace(saved.workflow(), &changed),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    changed = workflow.clone();
    changed.updated_at_millis += 1;
    changed.nodes[0].source.as_mut().expect("source").repository = "other/project".into();
    assert!(matches!(
        store.replace(saved.workflow(), &changed),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    changed = workflow.clone();
    changed.updated_at_millis += 1;
    changed.nodes[0].state = CloudJobState::Provisioning;
    let updated = store.replace(saved.workflow(), &changed).expect("progress update");
    assert_eq!(
        store
            .load_remote_allocation(OWNER, "workspace")
            .expect("recover")
            .expect("allocation")
            .workflow(),
        &updated
    );
}

#[test]
fn remote_node_protocol_rejects_extra_nodes_retries_and_missing_source_or_worker() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    let workflow = saved.workflow().workflow();
    let mut changed = workflow.clone();
    let mut extra = changed.nodes[0].clone();
    extra.id = CloudJobId::new();
    extra.logical_key = "another-node".into();
    extra.kind = WorkflowNodeKind::Build;
    changed.nodes.push(extra);
    assert!(changed.validate().is_err());
    for change in [0, 1, 2] {
        let mut changed = workflow.clone();
        match change {
            0 => changed.nodes[0].source = None,
            1 => changed.nodes[0].worker = None,
            _ => changed.nodes[0].retry.max_attempts = 2,
        }
        assert!(changed.validate().is_err());
    }
}

#[test]
fn cancelled_reconciling_or_observed_workspaces_cannot_receive_a_first_grant() {
    for change in [0, 1, 2] {
        let (_directory, store) = store();
        let saved = allocate(&store, &dormant(&store, "workspace"));
        let mut next = saved.workspace().state().clone();
        let worker = observed_worker(&next);
        let runtime = next.runtime.as_mut().expect("runtime");
        match change {
            0 => {
                runtime.cleanup = Some(RemoteCleanupIntent {
                    reason: RemoteCleanupReason::Cancelled,
                    requested_at_millis: 2000,
                });
            }
            1 => runtime.phase = RemoteRuntimePhase::Reconciling,
            _ => runtime.worker = Some(worker),
        }
        store
            .replace_remote_workspace(saved.workspace(), &next)
            .expect("record update");
        let workflow = saved.workflow().workflow();
        assert!(
            matches!(
                store.claim_worker_creation(
                    workflow.id,
                    workflow.nodes[0].id,
                    workflow.nodes[0].worker.as_ref().expect("target"),
                    "synthetic-worker"
                ),
                Err(CloudStoreError::ClaimTargetNotReady(_))
            ),
            "case {change}"
        );
        assert_eq!(counts(&store), (1, 1, 0));
        assert!(
            store
                .load_remote_allocation(OWNER, "workspace")
                .expect("recovery")
                .is_some()
        );
    }
}
