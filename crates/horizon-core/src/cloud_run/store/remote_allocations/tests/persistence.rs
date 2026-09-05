use super::*;
use crate::cloud_run::{ArtifactDigest, interactive_worker::InteractiveWorkerSshEndpoint};
use crate::remote_workspace::RepositoryCheckpoint;

#[test]
fn generic_workflow_updates_cannot_renew_expired_setup_authorization() {
    let (_directory, store) = store();
    let saved = store
        .allocate_remote_runtime(&dormant(&store, "workspace"), 1000, 2000)
        .expect("allocate");
    let mut extended = saved.workflow().workflow().clone();
    extended.updated_at_millis += 1;
    extended.retain_until_millis = current_unix_millis().expect("now") + 86_400_000;
    assert!(matches!(
        store.replace(saved.workflow(), &extended),
        Err(CloudStoreError::InvalidRemoteAllocation)
    ));
    assert_eq!(
        store.load(extended.id).expect("setup unchanged"),
        Some(saved.workflow().clone())
    );
    assert!(matches!(
        store.claim_worker_creation(
            extended.id,
            extended.nodes[0].id,
            &saved.workspace().state().spec.target,
            "synthetic-worker"
        ),
        Err(CloudStoreError::WorkflowExpired(_))
    ));
    assert_eq!(counts(&store), (1, 1, 0));
}

#[test]
fn setup_retention_is_independent_of_worker_lifetime_and_panel_intent_count() {
    for lifetime in [WorkerLifetime::Persistent, WorkerLifetime::TimeLimited { seconds: 900 }] {
        let (_directory, store) = store();
        let mut state = workspace("workspace");
        state.spec.target.lifetime = lifetime;
        state.spec.panels.clear();
        let dormant = store.create_remote_workspace(OWNER, &state).expect("empty intent");
        let saved = store
            .allocate_remote_runtime(&dormant, 1000, 2000)
            .expect("one second of setup authorization, independent of execution");
        let workflow = saved.workflow().workflow();
        assert_eq!(workflow.retain_until_millis, 2000);
        assert_eq!(workflow.nodes[0].worker.as_ref(), Some(&state.spec.target));
        assert_eq!(saved.workspace().state().spec.target.lifetime, lifetime);
        assert!(saved.workspace().state().spec.panels.is_empty());
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
        let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen");
        assert_eq!(
            reopened.load_remote_allocation(OWNER, "workspace").expect("recover"),
            Some(saved)
        );
        assert_eq!(counts(&reopened), (1, 1, 0));
    }
}

#[test]
fn clearing_panel_intents_neither_cancels_requested_setup_nor_renews_creation() {
    let (_directory, store) = store();
    let saved = allocate(&store, &dormant(&store, "workspace"));
    let mut next = saved.workspace().state().clone();
    next.spec.panels.clear();
    let cleared = store
        .replace_remote_workspace(saved.workspace(), &next)
        .expect("clear intents");
    assert_eq!(cleared.state().runtime, saved.workspace().state().runtime);
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
            .expect("no new grant")
    );
    assert!(matches!(
        reopened.allocate_remote_runtime(&cleared, 1000, 2000),
        Err(Error::RuntimeAlreadyActive)
    ));
    next.spec.panels.clone_from(&saved.workspace().state().spec.panels);
    let restored = reopened
        .replace_remote_workspace(&cleared, &next)
        .expect("restore intents");
    assert_eq!(restored.state().runtime, saved.workspace().state().runtime);
    assert!(
        !reopened
            .claim_worker_creation(workflow.id, node.id, target, "synthetic-worker")
            .expect("still no new grant")
    );
    assert_eq!(counts(&reopened), (1, 1, 1));
}

#[test]
fn expired_setup_reopens_persistent_worker_and_accepts_checkpoint_without_new_generation() {
    let (_directory, store) = store();
    let saved = store
        .allocate_remote_runtime(&dormant(&store, "workspace"), 1000, 2000)
        .expect("allocate");
    let mut next = saved.workspace().state().clone();
    let worker = observed_worker(&next);
    let runtime = next.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Ready;
    runtime.ssh = Some(InteractiveWorkerSshEndpoint {
        host: "127.0.0.1".into(),
        port: 2222,
        username: "horizon".into(),
        host_key: worker.ssh_public_key.clone(),
    });
    runtime.worker = Some(worker);
    let running = store
        .replace_remote_workspace(saved.workspace(), &next)
        .expect("verified worker fixture");
    let reopened = CloudWorkflowStore::open_path(store.path()).expect("reopen after setup expiry");
    assert!(
        reopened
            .list_retained(current_unix_millis().expect("now"))
            .expect("retained setup")
            .is_empty()
    );
    let recovered = reopened
        .load_remote_allocation(OWNER, "workspace")
        .expect("load persistent allocation")
        .expect("allocation");
    assert_eq!(recovered.workspace(), &running);
    assert_eq!(recovered.workflow(), saved.workflow());
    let workflow = saved.workflow().workflow();
    assert!(matches!(
        reopened.claim_worker_creation(workflow.id, workflow.nodes[0].id, &next.spec.target, "synthetic-worker"),
        Err(CloudStoreError::WorkflowExpired(_))
    ));
    next.checkpoint = Some(RepositoryCheckpoint {
        workspace_local_id: next.spec.workspace_local_id.clone(),
        base_commit: next.spec.repository.commit.clone(),
        manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
        runtime_generation: next.spec.generation,
        generation: 1,
        captured_at_millis: current_unix_millis().expect("checkpoint time"),
        recovery_artifact: None,
    });
    let checkpointed = reopened
        .replace_remote_workspace(&running, &next)
        .expect("checkpoint after setup expiry");
    assert_eq!(checkpointed.state().runtime, running.state().runtime);
    assert!(
        checkpointed
            .state()
            .checkpoint
            .as_ref()
            .expect("checkpoint")
            .captured_at_millis
            > workflow.retain_until_millis
    );
    assert!(matches!(
        reopened.allocate_remote_runtime(&checkpointed, 3000, 4000),
        Err(Error::RuntimeAlreadyActive)
    ));
    assert_eq!(
        reopened
            .load_remote_allocation(OWNER, "workspace")
            .expect("unchanged allocation")
            .expect("allocation")
            .workspace(),
        &checkpointed
    );
    assert_eq!(counts(&reopened), (1, 1, 0));
}
