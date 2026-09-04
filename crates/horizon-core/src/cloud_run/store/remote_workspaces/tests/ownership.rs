use super::*;

#[test]
fn records_survive_reopen_and_cannot_be_read_or_adopted_by_another_session() {
    let (_directory, store) = store();
    let state = workspace("workspace-one");
    let stored = store.create_remote_workspace(OWNER, &state).expect("create");
    assert_eq!(stored.state(), &state);
    assert_eq!(stored.session_id(), OWNER);
    assert_eq!(stored.revision(), 1);
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_workspace(OWNER, "workspace-one").expect("load"),
        Some(stored.clone())
    );
    assert_eq!(reopened.list_remote_workspaces(OWNER).expect("list"), vec![stored]);
    assert!(
        reopened
            .list_remote_workspaces(OTHER_OWNER)
            .expect("other session")
            .is_empty()
    );
    assert!(matches!(
        reopened.load_remote_workspace(OTHER_OWNER, "workspace-one"),
        Err(RemoteWorkspaceStoreError::OwnershipMismatch)
    ));
    assert!(matches!(
        reopened.create_remote_workspace(OTHER_OWNER, &state),
        Err(RemoteWorkspaceStoreError::AlreadyExists)
    ));
    assert!(
        reopened
            .load_remote_workspace(OWNER, "missing")
            .expect("missing")
            .is_none()
    );
}

#[test]
fn concurrent_writers_have_exactly_one_winner_and_preserve_the_winning_intent() {
    let (_directory, store) = store();
    let stored = store
        .create_remote_workspace(OWNER, &workspace("workspace"))
        .expect("create");
    let barrier = Arc::new(Barrier::new(3));
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let expected = stored.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let next = provisioning(expected.state().clone());
                barrier.wait();
                store.replace_remote_workspace(&expected, &next)
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
            .filter(|result| matches!(
                result,
                Err(RemoteWorkspaceStoreError::RevisionConflict { expected: 1, actual: 2 })
            ))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).expect("winner");
    assert_eq!(
        store.load_remote_workspace(OWNER, "workspace").expect("load winner"),
        Some(winner)
    );
}

#[test]
fn replacements_preserve_identity_runtime_and_checkpoint_watermarks() {
    let (_directory, store) = store();
    let mut state = provisioning(workspace("workspace"));
    state.checkpoint = Some(RepositoryCheckpoint {
        workspace_local_id: "workspace".into(),
        base_commit: state.spec.repository.commit.clone(),
        manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
        runtime_generation: 1,
        generation: 4,
        captured_at_millis: 1000,
        recovery_artifact: Some(ArtifactDigest::parse_sha256("d".repeat(64)).expect("artifact")),
    });
    let stored = store
        .create_remote_workspace(OWNER, &state)
        .expect("create active snapshot");
    let mut changed = state.clone();
    changed.runtime.as_mut().expect("runtime").workflow_id = CloudWorkflowId::new();
    assert!(matches!(
        store.replace_remote_workspace(&stored, &changed),
        Err(RemoteWorkspaceStoreError::ReplacementIdentityMismatch)
    ));
    changed = state.clone();
    changed.checkpoint = None;
    assert!(matches!(
        store.replace_remote_workspace(&stored, &changed),
        Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
    ));
    changed = state.clone();
    changed.checkpoint.as_mut().expect("checkpoint").manifest_digest =
        ArtifactDigest::parse_sha256("e".repeat(64)).expect("digest");
    assert!(matches!(
        store.replace_remote_workspace(&stored, &changed),
        Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
    ));
    changed = state;
    let runtime = changed.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 2000,
    });
    let cancelling = store
        .replace_remote_workspace(&stored, &changed)
        .expect("persist cleanup intent");
    let mut lost_cleanup = changed.clone();
    let runtime = lost_cleanup.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Provisioning;
    runtime.cleanup = None;
    assert!(matches!(
        store.replace_remote_workspace(&cancelling, &lost_cleanup),
        Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
    ));
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened
            .load_remote_workspace(OWNER, "workspace")
            .expect("load cleanup"),
        Some(cancelling.clone())
    );
    changed.runtime = None;
    let dormant = store
        .replace_remote_workspace(&cancelling, &changed)
        .expect("record already verified disposal");
    assert_eq!(dormant.state().checkpoint, cancelling.state().checkpoint);
    let mut reused = provisioning(changed.clone());
    reused.spec.generation = changed.spec.generation;
    reused.runtime.as_mut().expect("runtime").generation = changed.spec.generation;
    assert!(matches!(
        store.replace_remote_workspace(&dormant, &reused),
        Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
    ));
    store
        .replace_remote_workspace(&dormant, &provisioning(changed))
        .expect("next generation");
}

#[test]
fn pending_cleanup_preserves_its_exact_reason_and_request_time() {
    let (_directory, store) = store();
    let mut state = provisioning(workspace("workspace"));
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 2000,
    });
    let stored = store.create_remote_workspace(OWNER, &state).expect("pending cleanup");
    for (reason, requested_at_millis) in [
        (RemoteCleanupReason::LastPanelClosed, 2000),
        (RemoteCleanupReason::Cancelled, 1999),
        (RemoteCleanupReason::Cancelled, 2001),
    ] {
        let mut changed = state.clone();
        changed.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
            reason,
            requested_at_millis,
        });
        assert!(matches!(
            store.replace_remote_workspace(&stored, &changed),
            Err(RemoteWorkspaceStoreError::NonMonotonicReplacement)
        ));
    }
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_workspace(OWNER, "workspace").expect("load intent"),
        Some(stored.clone())
    );
    state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Failed;
    let failed = reopened
        .replace_remote_workspace(&stored, &state)
        .expect("phase update retains the same cleanup intent");
    assert_eq!(
        failed.state().runtime.as_ref().expect("runtime").cleanup,
        stored.state().runtime.as_ref().expect("runtime").cleanup
    );
}

fn expired_runtime() -> RemoteWorkspaceState {
    use crate::cloud_run::interactive_worker::{
        InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLease, InteractiveWorkerSshEndpoint,
    };
    let mut state = provisioning(workspace("workspace"));
    let key = public_key(7);
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Ready;
    runtime.worker = Some(InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: state.spec.target.provider,
            workflow_id: runtime.workflow_id,
            job_id: runtime.job_id,
            resource_id: "synthetic-exact-worker".into(),
        },
        target: state.spec.target.clone(),
        ssh_public_key: public_key(6),
        lease: InteractiveWorkerLease {
            terminate_after: "2020-01-01T00:00:00Z".into(),
        },
    });
    runtime.ssh = Some(InteractiveWorkerSshEndpoint {
        host: "127.0.0.1".into(),
        port: 2222,
        username: "developer".into(),
        host_key: key,
    });
    state
}

fn public_key(byte: u8) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

#[test]
fn expired_exact_worker_handles_and_pinned_endpoints_remain_durable_for_reconciliation() {
    let (_directory, store) = store();
    let mut state = expired_runtime();
    let stored = store
        .create_remote_workspace(OWNER, &state)
        .expect("store expired handle");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_workspace(OWNER, "workspace").expect("load"),
        Some(stored.clone())
    );
    state
        .runtime
        .as_mut()
        .expect("runtime")
        .worker
        .as_mut()
        .expect("worker")
        .identity
        .resource_id = "another-worker".into();
    assert!(matches!(
        reopened.replace_remote_workspace(&stored, &state),
        Err(RemoteWorkspaceStoreError::ReplacementIdentityMismatch)
    ));
}

#[test]
fn first_endpoint_can_be_pinned_but_never_replaced_within_the_same_runtime() {
    use crate::cloud_run::interactive_worker::InteractiveWorkerSshEndpoint;
    let (_directory, store) = store();
    let state = expired_runtime();
    let endpoint = state.runtime.as_ref().expect("runtime").ssh.as_ref().expect("endpoint");
    let mut unpinned = state.clone();
    let runtime = unpinned.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Reconciling;
    runtime.ssh = None;
    let pending = store
        .create_remote_workspace(OWNER, &unpinned)
        .expect("no endpoint yet");
    let pinned = store
        .replace_remote_workspace(&pending, &state)
        .expect("pin the first trusted endpoint");
    for changed_endpoint in [
        None,
        Some(InteractiveWorkerSshEndpoint {
            host: "192.0.2.44".into(),
            ..endpoint.clone()
        }),
        Some(InteractiveWorkerSshEndpoint {
            port: 2223,
            ..endpoint.clone()
        }),
        Some(InteractiveWorkerSshEndpoint {
            username: "another-user".into(),
            ..endpoint.clone()
        }),
        Some(InteractiveWorkerSshEndpoint {
            host_key: public_key(8),
            ..endpoint.clone()
        }),
    ] {
        let mut changed = state.clone();
        let runtime = changed.runtime.as_mut().expect("runtime");
        runtime.phase = RemoteRuntimePhase::Reconciling;
        runtime.ssh = changed_endpoint;
        changed.validate().expect("domain-valid observation");
        assert!(matches!(
            store.replace_remote_workspace(&pinned, &changed),
            Err(RemoteWorkspaceStoreError::ReplacementIdentityMismatch)
        ));
    }
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
    assert_eq!(
        reopened.load_remote_workspace(OWNER, "workspace").expect("load pin"),
        Some(pinned.clone())
    );
    unpinned.runtime.as_mut().expect("runtime").ssh = Some(endpoint.clone());
    reopened
        .replace_remote_workspace(&pinned, &unpinned)
        .expect("reconciliation retains the same endpoint");
}
