use super::*;
use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase};

fn retained_key(fixture: &Fixture) -> crate::remote_ssh_identity::RemoteSshIdentity {
    let runtime = fixture.reload();
    let runtime = runtime.workspace().state().runtime.as_ref().expect("runtime");
    fixture
        .identities
        .prepare_new(runtime.workflow_id, runtime.job_id)
        .expect("key")
}

fn expire_setup(fixture: &Fixture) {
    let mut workflow = fixture.reload().workflow().workflow().clone();
    workflow.created_at_millis = 1000;
    workflow.updated_at_millis = 1000;
    workflow.retain_until_millis = 2000;
    rusqlite::Connection::open(fixture.store.path())
        .expect("database")
        .execute(
            "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000, retain_until_millis=2000,
             snapshot=?1 WHERE workflow_id=?2",
            rusqlite::params![
                serde_json::to_vec(&workflow).expect("snapshot"),
                workflow.id.to_string()
            ],
        )
        .expect("expired fixture");
}

#[test]
fn explicit_start_retains_one_allocation_request_and_key_without_claiming_task_readiness() {
    let fixture = Fixture::new();
    let provider = fixture.provider();
    let saved = start_remote_workspace(
        &fixture.store,
        &fixture.identities,
        &provider,
        &fixture.dormant,
        i64::MAX,
    )
    .expect("start");
    let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
    assert_eq!(runtime.phase, RemoteRuntimePhase::Reconciling);
    assert!(runtime.worker.is_some());
    assert!(runtime.ssh.is_some());
    assert!(saved.workspace().state().spec.panels.is_empty());
    assert_eq!(fixture.counts(), [1, 1, 1]);
    assert_eq!(provider.calls(), [1, 1, 0, 0, 0]);
    assert_eq!(fixture.retry(&provider).expect("noncreating retry"), saved);
    assert_eq!(provider.calls(), [1, 1, 0, 1, 0]);
    assert!(
        start_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            &fixture.dormant,
            i64::MAX
        )
        .is_err()
    );
    assert_eq!(fixture.counts(), [1, 1, 1]);
}

#[test]
fn invalid_setup_retention_is_actionable_without_allocating_or_preparing_keys() {
    let fixture = Fixture::new();
    let provider = fixture.provider();
    assert_eq!(
        start_remote_workspace(&fixture.store, &fixture.identities, &provider, &fixture.dormant, 0),
        Err(RemoteWorkspaceSetupError::InvalidAllocationRetention)
    );
    assert_eq!(fixture.counts(), [0; 3]);
    assert_eq!(provider.calls(), [0; 5]);
    assert!(!fixture.directory.path().join("home/remote-ssh-identities").exists());
}

#[test]
fn an_existing_allocation_requires_recovery_instead_of_reporting_storage_failure() {
    let fixture = Fixture::new();
    let allocation = fixture.allocate();
    let provider = fixture.provider();
    assert_eq!(
        start_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            allocation.workspace(),
            i64::MAX
        ),
        Err(RemoteWorkspaceSetupError::RuntimeAlreadyActive)
    );
    assert_eq!(fixture.reload(), allocation);
    assert_eq!(fixture.counts(), [1, 1, 0]);
    assert_eq!(provider.calls(), [0; 5]);
    assert!(!fixture.directory.path().join("home/remote-ssh-identities").exists());
}

#[test]
fn interruptions_before_key_reservation_or_creation_reuse_the_exact_generation() {
    for interruption in 0..3 {
        let fixture = Fixture::new();
        let allocation = fixture.allocate();
        let key = (interruption > 0).then(|| retained_key(&fixture));
        let bytes = key
            .as_ref()
            .map(|key| std::fs::read(key.private_key_path()).expect("key bytes"));
        if interruption == 2 {
            fixture
                .store
                .reserve_remote_worker_request(&allocation, key.as_ref().expect("key").public_key())
                .expect("reserve");
        }
        let provider = fixture.provider();
        let saved = fixture.retry(&provider).expect("resume setup");
        assert_eq!(saved.workflow(), allocation.workflow());
        if let Some(key) = key {
            assert_eq!(
                saved.worker_request().expect("request").ssh_public_key,
                key.public_key()
            );
            assert_eq!(
                std::fs::read(key.private_key_path()).expect("retained bytes"),
                bytes.expect("bytes")
            );
        }
        assert_eq!(fixture.counts(), [1, 1, 1]);
        assert_eq!(provider.calls(), [1, 1, 0, 0, 0]);
    }
}

#[test]
fn lost_create_response_reopens_without_a_second_ensure_or_replacement() {
    let fixture = Fixture::new();
    let provider = fixture.provider();
    provider.fail_response.store(true, Ordering::SeqCst);
    let error = start_remote_workspace(
        &fixture.store,
        &fixture.identities,
        &provider,
        &fixture.dormant,
        i64::MAX,
    )
    .expect_err("lost response");
    assert_eq!(error, RemoteWorkspaceSetupError::ProviderUnavailable);
    assert!(!format!("{error:?} {error}").contains("synthetic-private-provider-response"));
    let before = fixture.reload();
    assert!(
        before
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_none()
    );
    let request = before.worker_request().expect("retained request");
    let key = fixture
        .identities
        .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
        .expect("key");
    let bytes = std::fs::read(key.private_key_path()).expect("key bytes");
    let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("reopen");
    let recovered = retry_remote_workspace_setup(&reopened, &fixture.identities, &provider, &before).expect("recover");
    assert_eq!(recovered.worker_request().expect("same request"), request);
    assert_eq!(provider.calls(), [1, 1, 1, 0, 0]);
    *provider.observation.lock().expect("observation") = None;
    assert_eq!(fixture.retry(&provider).expect("absence is not recreation"), recovered);
    assert_eq!(provider.calls(), [1, 1, 1, 1, 0]);
    assert_eq!(fixture.counts(), [1, 1, 1]);
    assert_eq!(std::fs::read(key.private_key_path()).expect("retained"), bytes);
}

#[test]
fn an_already_claimed_absent_worker_never_crosses_the_ensure_boundary() {
    let fixture = Fixture::new();
    let allocation = fixture.allocate();
    let key = retained_key(&fixture);
    let reserved = fixture
        .store
        .reserve_remote_worker_request(&allocation, key.public_key())
        .expect("reserve");
    let request = reserved.worker_request().expect("request");
    assert!(
        fixture
            .store
            .claim_worker_creation(request.workflow_id, request.job_id, &request.target, "synthetic-worker")
            .expect("claim")
    );
    let provider = fixture.provider();
    fixture.retry(&provider).expect("reconcile absent");
    assert_eq!(provider.calls(), [0, 0, 1, 0, 0]);
    assert_eq!(fixture.counts(), [1, 1, 1]);
}

#[test]
fn missing_or_mismatched_reserved_private_keys_fail_without_regeneration() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        let allocation = fixture.allocate();
        let key = retained_key(&fixture);
        let saved = fixture
            .store
            .reserve_remote_worker_request(&allocation, key.public_key())
            .expect("reserve");
        let expected = if missing {
            std::fs::remove_file(key.private_key_path()).expect("remove fixture key");
            RemoteSshIdentityError::Missing
        } else {
            let other = fixture
                .identities
                .prepare_new(
                    crate::cloud_run::CloudWorkflowId::new(),
                    crate::cloud_run::CloudJobId::new(),
                )
                .expect("other key");
            std::fs::write(
                key.private_key_path(),
                std::fs::read(other.private_key_path()).expect("other bytes"),
            )
            .expect("mismatch fixture");
            RemoteSshIdentityError::Mismatch
        };
        let provider = fixture.provider();
        assert_eq!(fixture.retry(&provider), Err(expected.into()));
        assert_eq!(fixture.reload(), saved);
        assert_eq!(provider.calls(), [0; 5]);
        assert_eq!(fixture.counts(), [1, 1, 0]);
        if missing {
            assert!(!key.private_key_path().exists());
        }
    }
}

#[test]
fn setup_management_and_snapshot_changes_fail_closed_without_provider_cleanup() {
    let fixture = Fixture::new();
    let allocation = fixture.allocate();
    let provider = fixture.provider();
    let mut state = allocation.workspace().state().clone();
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(allocation.workspace(), &state)
        .expect("intent");
    assert_eq!(
        retry_remote_workspace_setup(&fixture.store, &fixture.identities, &provider, &allocation),
        Err(RemoteWorkspaceRecoveryError::StateChanged.into())
    );
    assert_eq!(
        fixture.retry(&provider),
        Err(RemoteWorkspaceRecoveryError::ManagementPending.into())
    );
    assert_eq!(provider.calls(), [0; 5]);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn a_late_provider_response_cannot_overwrite_newer_workspace_intent() {
    let fixture = Fixture::new();
    let mut provider = fixture.provider();
    let store = fixture.store.clone();
    provider.after_create = Some(Box::new(move || {
        let allocation = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation");
        let mut state = allocation.workspace().state().clone();
        state.spec.working_directory = "changed".into();
        store
            .replace_remote_workspace(allocation.workspace(), &state)
            .expect("user edit");
    }));
    assert_eq!(
        start_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            &fixture.dormant,
            i64::MAX
        ),
        Err(RemoteWorkspaceRecoveryError::StateChanged.into())
    );
    assert_eq!(fixture.reload().workspace().state().spec.working_directory, "changed");
    assert!(
        fixture
            .reload()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_none()
    );
    assert_eq!(provider.calls(), [1, 1, 0, 0, 0]);
    fixture.retry(&provider).expect("recover only");
    assert_eq!(provider.calls(), [1, 1, 1, 0, 0]);
}

#[test]
fn concurrent_explicit_starts_allocate_once_and_concurrent_retries_create_once() {
    for retry in [false, true] {
        let fixture = Fixture::new();
        if retry {
            let allocation = fixture.allocate();
            let key = retained_key(&fixture);
            fixture
                .store
                .reserve_remote_worker_request(&allocation, key.public_key())
                .expect("reserve");
        }
        let expected = retry.then(|| fixture.reload());
        let provider = fixture.provider();
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let run = || {
                barrier.wait();
                if let Some(expected) = &expected {
                    retry_remote_workspace_setup(&fixture.store, &fixture.identities, &provider, expected)
                } else {
                    start_remote_workspace(
                        &fixture.store,
                        &fixture.identities,
                        &provider,
                        &fixture.dormant,
                        i64::MAX,
                    )
                }
            };
            let first = scope.spawn(run);
            let second = scope.spawn(run);
            [first.join().expect("first"), second.join().expect("second")]
        });
        assert!(results.iter().any(Result::is_ok));
        assert_eq!(fixture.counts(), [1, 1, 1]);
        assert_eq!(provider.calls()[1], 1);
        assert_eq!(provider.calls()[4], 0);
    }
}

#[test]
fn setup_expiry_never_renews_creation_or_prevents_recovery_of_a_persistent_worker() {
    for created in [false, true] {
        let fixture = Fixture::new();
        let provider = fixture.provider();
        if created {
            provider.fail_response.store(true, Ordering::SeqCst);
            assert!(
                start_remote_workspace(
                    &fixture.store,
                    &fixture.identities,
                    &provider,
                    &fixture.dormant,
                    i64::MAX
                )
                .is_err()
            );
        } else {
            fixture.allocate();
        }
        expire_setup(&fixture);
        let before = fixture.reload();
        if created {
            let recovered = fixture.retry(&provider).expect("persistent recovery");
            assert_eq!(recovered.workflow(), before.workflow());
            assert_eq!(provider.calls(), [1, 1, 1, 0, 0]);
            assert_eq!(fixture.counts(), [1, 1, 1]);
        } else {
            assert_eq!(
                fixture.retry(&provider),
                Err(RemoteWorkspaceRecoveryError::MissingRequest.into())
            );
            assert_eq!(fixture.reload(), before);
            assert_eq!(provider.calls(), [0; 5]);
            assert_eq!(fixture.counts(), [1, 1, 0]);
        }
    }
}

#[test]
fn wrong_observation_preserves_the_request_and_consumed_grant_without_cleanup() {
    let fixture = Fixture::new();
    let mut provider = fixture.provider();
    provider.invalid_observation = true;
    assert_eq!(
        start_remote_workspace(
            &fixture.store,
            &fixture.identities,
            &provider,
            &fixture.dormant,
            i64::MAX
        ),
        Err(RemoteWorkspaceRecoveryError::InvalidObservation.into())
    );
    let saved = fixture.reload();
    assert!(saved.worker_request().is_ok());
    assert!(
        saved
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_none()
    );
    assert_eq!(provider.calls(), [1, 1, 0, 0, 0]);
    assert_eq!(fixture.counts(), [1, 1, 1]);
    fixture.retry(&provider).expect("noncreating recovery of actual worker");
    assert_eq!(provider.calls(), [1, 1, 1, 0, 0]);
}

#[test]
fn stale_workflow_snapshot_is_rejected_before_key_preparation_or_ensure() {
    let fixture = Fixture::new();
    let allocation = fixture.allocate();
    let mut workflow = allocation.workflow().workflow().clone();
    workflow.title = "Updated intent".into();
    workflow.updated_at_millis += 1;
    fixture
        .store
        .replace(allocation.workflow(), &workflow)
        .expect("workflow edit");
    let provider = fixture.provider();
    assert_eq!(
        retry_remote_workspace_setup(&fixture.store, &fixture.identities, &provider, &allocation),
        Err(RemoteWorkspaceRecoveryError::StateChanged.into())
    );
    assert!(!fixture.directory.path().join("home/remote-ssh-identities").exists());
    assert_eq!(provider.calls(), [0; 5]);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn failed_identity_preparation_rechecks_expiry_without_losing_an_available_setups_error() {
    for expired in [false, true] {
        let fixture = Fixture::new();
        let allocation = fixture.allocate();
        assert!(setup_available(&fixture.store, &allocation).expect("admitted"));
        let key = retained_key(&fixture);
        let bytes = std::fs::read(key.private_key_path()).expect("retained key");
        let provider = fixture.provider();
        if expired {
            expire_setup(&fixture);
        }
        let current = fixture.reload();
        let error = if expired {
            prepare_identity(&fixture.store, &fixture.identities, &current).expect_err("reservation after expiry")
        } else {
            RemoteSshIdentityError::KeyUtilityFailed.into()
        };
        let expected = if expired {
            RemoteWorkspaceRecoveryError::MissingRequest.into()
        } else {
            RemoteSshIdentityError::KeyUtilityFailed.into()
        };
        assert_eq!(
            recover_preparation_failure(&fixture.store, &fixture.identities, &provider, &current, error),
            Err(expected)
        );
        assert_eq!(fixture.reload(), current);
        assert_eq!(std::fs::read(key.private_key_path()).expect("retained key"), bytes);
        assert_eq!(fixture.counts(), [1, 1, 0]);
        assert_eq!(provider.calls(), [0; 5]);
    }
}
