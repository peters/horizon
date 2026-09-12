use super::*;
use crate::{
    cloud_run::runpod::{RunPodApiKey, RunPodProfile},
    remote_environment_observation::configured::runpod_with,
};
use std::cell::Cell;

fn profile() -> RunPodProfile {
    serde_json::from_value(serde_json::json!({
        "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
        "ports":["22/tcp"], "volume_gib":10, "data_center_id":"synthetic-dc"
    }))
    .expect("profile")
}

fn fixture(network: bool) -> Fixture {
    let fixture = Fixture::with_provider(WorkerLifetime::Persistent, true, CloudProvider::RunPod, network);
    fixture.retain(&fixture.status());
    fixture
}

fn key() -> Result<RunPodApiKey, ConfiguredObservationError> {
    RunPodApiKey::new("synthetic-provider-credential")
        .map_err(|_| ConfiguredObservationError::RunPodCredentialUnavailable)
}

fn checkpoint(fixture: &Fixture) {
    let before = fixture.reload();
    let mut state = before.workspace().state().clone();
    state.checkpoint = Some(RepositoryCheckpoint {
        workspace_local_id: "workspace".into(),
        base_commit: state.spec.repository.commit.clone(),
        manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
        runtime_generation: 1,
        generation: 1,
        captured_at_millis: 1,
        recovery_artifact: None,
    });
    fixture
        .store
        .replace_remote_workspace(before.workspace(), &state)
        .expect("checkpoint");
}

#[test]
fn retained_lifecycle_and_absence_require_no_private_key_or_state_mutation() {
    for network in [false, true] {
        let fixture = fixture(network);
        let before = fixture.reload();
        for lifecycle in [
            Some(InteractiveWorkerLifecycle::Ready),
            Some(InteractiveWorkerLifecycle::Stopped),
            Some(InteractiveWorkerLifecycle::Failed),
            Some(InteractiveWorkerLifecycle::Unknown),
            None,
        ] {
            let status = lifecycle.map(|lifecycle| {
                let mut status = fixture.status();
                status.lifecycle = lifecycle;
                if lifecycle != InteractiveWorkerLifecycle::Ready {
                    status.ssh = None;
                }
                status
            });
            let mut provider = Provider::new(status);
            provider.kind = CloudProvider::RunPod;
            let result = runpod_with(
                &fixture.store,
                &profile(),
                &before.workspace().environment_summary(),
                key,
                |_, workspace| observe_remote_environment(&fixture.store, &provider, workspace),
            )
            .expect("observation");
            assert_eq!(result.worker.map(|worker| worker.lifecycle), lifecycle);
            assert_eq!(provider.calls(), ["inspect"]);
            assert_eq!(fixture.reload(), before);
            assert_eq!(fixture.counts(), [1, 1, 0]);
        }
    }
}

#[test]
fn local_binding_refusals_never_load_credentials_or_query_provider() {
    for fault in 0..6 {
        let fixture = Fixture::with_provider(
            WorkerLifetime::Persistent,
            fault != 1,
            if fault == 0 {
                CloudProvider::Azure
            } else {
                CloudProvider::RunPod
            },
            fault == 5,
        );
        if fault != 1 && fault != 2 {
            let mut status = fixture.status();
            if fault == 3 {
                status.lifecycle = InteractiveWorkerLifecycle::Provisioning;
                status.ssh = None;
            }
            fixture.retain(&status);
        }
        let mut profile = profile();
        if fault == 4 {
            profile.name = "different".into();
        }
        if fault == 5 {
            profile.data_center_id = Some("different-dc".into());
        }
        let before = fixture.reload();
        assert!(
            runpod_with::<()>(
                &fixture.store,
                &profile,
                &before.workspace().environment_summary(),
                || panic!("credentials must remain unread"),
                |_, _| panic!("provider must remain unused"),
            )
            .is_err()
        );
        assert_eq!(fixture.reload(), before);
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }
}

#[test]
fn selection_and_credential_callback_drift_are_rejected_before_provider_query() {
    let fixture = fixture(true);
    let before = fixture.reload();
    let mut expected = before.workspace().environment_summary();
    expected.panel_count += 1;
    assert_eq!(
        runpod_with::<()>(
            &fixture.store,
            &profile(),
            &expected,
            || panic!("stale row must not read credentials"),
            |_, _| panic!("stale row must not query provider"),
        ),
        Err(ConfiguredObservationError::Observation(Error::StateChanged))
    );
    let queries = Cell::new(0);
    assert_eq!(
        runpod_with(
            &fixture.store,
            &profile(),
            &before.workspace().environment_summary(),
            || {
                checkpoint(&fixture);
                key()
            },
            |_, _| {
                queries.set(queries.get() + 1);
                Ok(())
            },
        ),
        Err(ConfiguredObservationError::Observation(Error::StateChanged))
    );
    assert_eq!(queries.get(), 0);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn observation_callback_drift_discards_result_without_overwriting_new_state() {
    let fixture = fixture(false);
    let before = fixture.reload();
    assert_eq!(
        runpod_with(
            &fixture.store,
            &profile(),
            &before.workspace().environment_summary(),
            key,
            |_, _| {
                checkpoint(&fixture);
                Ok(())
            },
        ),
        Err(ConfiguredObservationError::Observation(Error::StateChanged))
    );
    assert!(fixture.reload().workspace().state().checkpoint.is_some());
}

#[test]
fn pending_management_and_expired_setup_remain_read_only_observable() {
    let fixture = fixture(true);
    let before = fixture.reload();
    let mut workflow = before.workflow().workflow().clone();
    workflow.created_at_millis = 1000;
    workflow.updated_at_millis = 1000;
    workflow.retain_until_millis = 2000;
    rusqlite::Connection::open(fixture.store.path())
        .expect("database")
        .execute(
            "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
         retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
            rusqlite::params![
                serde_json::to_vec(&workflow).expect("snapshot"),
                workflow.id.to_string()
            ],
        )
        .expect("expired fixture");
    for phase in [RemoteRuntimePhase::Cancelling, RemoteRuntimePhase::Deleting] {
        let before = fixture.reload();
        let mut state = before.workspace().state().clone();
        let runtime = state.runtime.as_mut().expect("runtime");
        runtime.phase = phase;
        runtime.cleanup = Some(RemoteCleanupIntent {
            reason: RemoteCleanupReason::Cancelled,
            requested_at_millis: 1,
        });
        fixture
            .store
            .replace_remote_workspace(before.workspace(), &state)
            .expect("management");
        let before = fixture.reload();
        let mut provider = Provider::new(Some(fixture.status()));
        provider.kind = CloudProvider::RunPod;
        runpod_with(
            &fixture.store,
            &profile(),
            &before.workspace().environment_summary(),
            key,
            |_, workspace| observe_remote_environment(&fixture.store, &provider, workspace),
        )
        .expect("observation");
        assert_eq!(provider.calls(), ["inspect"]);
        assert_eq!(fixture.reload(), before);
        assert_eq!(before.workflow().workflow().retain_until_millis, 2000);
    }
}

#[test]
fn credential_failure_is_fixed_and_never_falls_back() {
    let fixture = fixture(false);
    let before = fixture.reload();
    let error = runpod_with::<()>(
        &fixture.store,
        &profile(),
        &before.workspace().environment_summary(),
        || Err(ConfiguredObservationError::RunPodCredentialUnavailable),
        |_, _| panic!("missing credential must not query provider"),
    )
    .expect_err("credential missing");
    assert_eq!(error, ConfiguredObservationError::RunPodCredentialUnavailable);
    assert!(!format!("{error:?} {error}").contains("synthetic-provider-credential"));
    assert_eq!(fixture.reload(), before);
}

#[test]
fn ordinary_retained_time_limited_worker_can_be_observed_without_renewal() {
    let fixture = Fixture::with_provider(
        WorkerLifetime::TimeLimited { seconds: 300 },
        true,
        CloudProvider::RunPod,
        false,
    );
    let status = fixture.status();
    fixture.retain(&status);
    let before = fixture.reload();
    let mut provider = Provider::new(Some(status));
    provider.kind = CloudProvider::RunPod;
    runpod_with(
        &fixture.store,
        &profile(),
        &before.workspace().environment_summary(),
        key,
        |_, workspace| observe_remote_environment(&fixture.store, &provider, workspace),
    )
    .expect("bounded worker metadata remains observable");
    assert_eq!(provider.calls(), ["inspect"]);
    assert_eq!(fixture.reload(), before);
}

#[test]
fn workflow_and_separate_selection_drift_are_fenced_at_both_callbacks() {
    for selection_change in [false, true] {
        for during_credential in [false, true] {
            let fixture = fixture(true);
            let before = fixture.reload();
            let change = || {
                if selection_change {
                    // The immutable selection is a separate row, outside allocation revisions.
                    let mut connection = rusqlite::Connection::open(fixture.store.path()).expect("database");
                    let transaction = connection.transaction().expect("atomic corruption fixture");
                    let trigger: String = transaction
                        .query_row(
                            "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                            [],
                            |row| row.get(0),
                        )
                        .expect("immutable trigger");
                    transaction
                        .execute_batch("DROP TRIGGER remote_network_volume_selections_no_update")
                        .expect("fixture only");
                    transaction
                        .execute("UPDATE remote_network_volume_selections SET volume_id='changed-volume' WHERE workspace_local_id='workspace'", [])
                        .expect("fixture selection change");
                    transaction.execute_batch(&trigger).expect("restore exact schema");
                    transaction.commit().expect("commit fixture change");
                } else {
                    let mut workflow = before.workflow().workflow().clone();
                    workflow.updated_at_millis += 1;
                    fixture
                        .store
                        .replace(before.workflow(), &workflow)
                        .expect("workflow change");
                }
            };
            let queries = Cell::new(0);
            let result = runpod_with(
                &fixture.store,
                &profile(),
                &before.workspace().environment_summary(),
                || {
                    if during_credential {
                        change();
                    }
                    key()
                },
                |_, _| {
                    queries.set(queries.get() + 1);
                    if !during_credential {
                        change();
                    }
                    Ok(())
                },
            );
            assert_eq!(
                result,
                Err(ConfiguredObservationError::Observation(Error::StateChanged))
            );
            assert_eq!(queries.get(), usize::from(!during_credential));
            let after = fixture.reload();
            assert_eq!(after.workspace(), before.workspace());
            if selection_change {
                assert_eq!(after, before, "selection drift does not change allocation");
                assert_eq!(
                    fixture
                        .store
                        .load_remote_network_volume_selection(&after)
                        .expect("selection")
                        .expect("retained")
                        .volume_id,
                    "changed-volume"
                );
            } else {
                assert_ne!(after.workflow(), before.workflow());
            }
            assert_eq!(fixture.counts(), [1, 1, 0]);
        }
    }
}
