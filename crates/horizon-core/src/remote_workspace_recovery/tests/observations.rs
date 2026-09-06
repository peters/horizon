use super::*;
use crate::cloud_run::{CloudJobId, CloudWorkflowId, interactive_worker::InteractiveWorkerLease};

#[test]
fn absent_and_nonready_observations_preserve_identity_and_seal_creation_without_ready_promotion() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let initial = fixture.reload();
    let mut status = fixture.status();
    let discovered = fixture
        .store
        .record_remote_worker_recovery(&initial, Some(&status))
        .expect("discovered");
    assert_eq!(discovered.workflow(), initial.workflow());
    assert_eq!(discovered.workspace().state().spec, initial.workspace().state().spec);
    assert_eq!(
        discovered.workspace().state().runtime.as_ref().expect("runtime").phase,
        RemoteRuntimePhase::Reconciling
    );
    for lifecycle in [
        InteractiveWorkerLifecycle::Stopped,
        InteractiveWorkerLifecycle::Failed,
        InteractiveWorkerLifecycle::Deleting,
        InteractiveWorkerLifecycle::Unknown,
        InteractiveWorkerLifecycle::Provisioning,
    ] {
        status.lifecycle = lifecycle;
        status.ssh = None;
        assert_eq!(
            fixture
                .store
                .record_remote_worker_recovery(&discovered, Some(&status))
                .expect("observed"),
            discovered
        );
    }
    assert_eq!(
        fixture
            .store
            .record_remote_worker_recovery(&discovered, None)
            .expect("absent"),
        discovered
    );
    assert!(
        fixture
            .store
            .claim_worker_creation(
                status.worker.identity.workflow_id,
                status.worker.identity.job_id,
                &status.worker.target,
                "synthetic-worker"
            )
            .is_err()
    );
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn wrong_observations_never_persist_any_part_of_the_response() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let saved = fixture.reload();
    for fault in 0..12 {
        let mut status = fixture.status();
        match fault {
            0 => status.worker.identity.workflow_id = CloudWorkflowId::new(),
            1 => status.worker.identity.job_id = CloudJobId::new(),
            2 => status.worker.identity.provider = CloudProvider::RunPod,
            3 => status.worker.identity.resource_id = "invalid resource".into(),
            4 => status.worker.target.profile = "different".into(),
            5 => status.worker.target.disk_gib += 1,
            6 => status.worker.ssh_public_key = public_key(2),
            7 => {
                status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                    terminate_after: "2020-01-01T00:00:00Z".into(),
                });
            }
            8 => status.ssh = None,
            9 => status.ssh.as_mut().expect("ssh").host_key = "untrusted".into(),
            10 => status.ssh.as_mut().expect("ssh").port = 0,
            11 => status.worker.target.lifetime = WorkerLifetime::TimeLimited { seconds: 3600 },
            _ => unreachable!(),
        }
        assert!(
            matches!(
                fixture.store.record_remote_worker_recovery(&saved, Some(&status)),
                Err(RemoteWorkspaceStoreError::InvalidWorkerObservation)
            ),
            "fault {fault}"
        );
        assert_eq!(fixture.reload(), saved);
    }
}

#[test]
fn exact_saved_worker_and_full_host_pin_cannot_rotate_on_recovery() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let original = fixture.status();
    let saved = fixture
        .store
        .record_remote_worker_recovery(&fixture.reload(), Some(&original))
        .expect("saved");
    for fault in 0..5 {
        let mut changed = original.clone();
        match fault {
            0 => changed.worker.identity.resource_id = "different-worker".into(),
            1 => changed.ssh.as_mut().expect("ssh").host_key = public_key(8),
            2 => changed.ssh.as_mut().expect("ssh").host = "127.0.0.2".into(),
            3 => changed.ssh.as_mut().expect("ssh").port = 2223,
            4 => changed.ssh.as_mut().expect("ssh").username = "different".into(),
            _ => unreachable!(),
        }
        assert!(matches!(
            fixture.store.record_remote_worker_recovery(&saved, Some(&changed)),
            Err(RemoteWorkspaceStoreError::InvalidWorkerObservation)
        ));
        assert_eq!(fixture.reload(), saved);
    }
}

#[test]
fn stale_workspace_or_workflow_snapshot_cannot_commit_an_observation() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let old = fixture.reload();
    let status = fixture.status();
    let mut workflow = old.workflow().workflow().clone();
    workflow.updated_at_millis += 1;
    fixture.store.replace(old.workflow(), &workflow).expect("workflow edit");
    let current = fixture.reload();
    assert_eq!(current.workspace(), old.workspace());
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&old, Some(&status)),
        Err(RemoteWorkspaceStoreError::SnapshotConflict)
    ));
    assert_eq!(fixture.reload(), current);
    fixture.cancel();
    let cancelled = fixture.reload();
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&current, Some(&status)),
        Err(RemoteWorkspaceStoreError::SnapshotConflict)
    ));
    assert!(matches!(
        fixture.store.record_remote_worker_recovery(&cancelled, Some(&status)),
        Err(RemoteWorkspaceStoreError::RuntimeRecoveryUnavailable)
    ));
    assert_eq!(fixture.reload(), cancelled);
}

#[test]
fn time_limited_readiness_uses_current_time_while_stopped_expired_identity_is_retained() {
    let fixture = Fixture::with_lifetime(WorkerLifetime::TimeLimited { seconds: 3600 });
    fixture.reserve(&public_key(1));
    let saved = fixture.reload();
    let mut status = fixture.status();
    for seconds in [-600, 7200] {
        let deadline = time::OffsetDateTime::now_utc() + time::Duration::seconds(seconds);
        status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: deadline
                .format(&time::format_description::well_known::Rfc3339)
                .expect("time"),
        });
        assert!(matches!(
            fixture.store.record_remote_worker_recovery(&saved, Some(&status)),
            Err(RemoteWorkspaceStoreError::InvalidWorkerObservation)
        ));
        assert_eq!(fixture.reload(), saved);
    }
    status.worker.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: "2020-01-01T00:00:00Z".into(),
    });
    status.lifecycle = InteractiveWorkerLifecycle::Stopped;
    status.ssh = None;
    let stopped = fixture
        .store
        .record_remote_worker_recovery(&saved, Some(&status))
        .expect("exact stopped identity");
    assert_eq!(
        stopped
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .as_ref(),
        Some(&status.worker)
    );
}

#[test]
fn reconciliation_preserves_saved_panels_checkpoints_and_does_not_preserve_stale_readiness() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let status = fixture.status();
    let saved = fixture
        .store
        .record_remote_worker_recovery(&fixture.reload(), Some(&status))
        .expect("saved");
    let mut state = saved.workspace().state().clone();
    state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Ready;
    state.spec.panels.push(crate::remote_workspace::RemotePanelBinding {
        panel_local_id: "panel-one".into(),
        kind: crate::PanelKind::Shell,
        command: None,
        working_directory: None,
        task_handoff: Some("Retain this task".into()),
        agent_session_id: None,
    });
    state.checkpoint = Some(crate::remote_workspace::RepositoryCheckpoint {
        workspace_local_id: "workspace".into(),
        base_commit: state.spec.repository.commit.clone(),
        manifest_digest: crate::cloud_run::ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
        runtime_generation: 1,
        generation: 1,
        captured_at_millis: 1000,
        recovery_artifact: None,
    });
    fixture
        .store
        .replace_remote_workspace(saved.workspace(), &state)
        .expect("ready fixture");
    let recovered = fixture
        .store
        .record_remote_worker_recovery(&fixture.reload(), None)
        .expect("missing worker");
    let recovered_state = recovered.workspace().state();
    assert_eq!(recovered_state.spec, state.spec);
    assert_eq!(recovered_state.checkpoint, state.checkpoint);
    let runtime = recovered_state.runtime.as_ref().expect("runtime");
    assert_eq!(runtime.phase, RemoteRuntimePhase::Reconciling);
    assert_eq!(runtime.worker, state.runtime.as_ref().expect("old runtime").worker);
    assert_eq!(runtime.ssh, state.runtime.as_ref().expect("old runtime").ssh);
}

#[test]
fn competing_observations_have_one_committed_identity() {
    let fixture = Fixture::new();
    fixture.reserve(&public_key(1));
    let expected = fixture.reload();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = (0..2)
        .map(|index| {
            let store = fixture.store.clone();
            let expected = expected.clone();
            let mut status = fixture.status();
            status.worker.identity.resource_id = format!("synthetic-worker-{index}");
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.record_remote_worker_recovery(&expected, Some(&status))
            })
        })
        .collect();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().expect("writer"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(RemoteWorkspaceStoreError::SnapshotConflict)))
            .count(),
        1
    );
    assert_eq!(
        &fixture.reload(),
        results.iter().find_map(|result| result.as_ref().ok()).expect("winner")
    );
    assert_eq!(fixture.counts(), [1, 1, 0]);
}
