use super::super::{InventoryAction, InventoryPage, RemoteEnvironments, paint};
use super::*;
use horizon_core::{
    cloud_run::{
        CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, interactive_worker::InteractiveWorkerIdentity,
    },
    remote_environment_observation::ObservedRemoteWorker,
    remote_workspace::RemoteRuntimePhase,
};

fn summary() -> RemoteEnvironmentSummary {
    RemoteEnvironmentSummary {
        workspace_local_id: "synthetic-workspace".into(),
        owning_session_id: "00000000-0000-4000-8000-000000000001".into(),
        revision: 1,
        repository: "example/project".into(),
        provider: CloudProvider::LocalDocker,
        profile: "development".into(),
        lifetime: WorkerLifetime::Persistent,
        generation: 1,
        saved_phase: Some(RemoteRuntimePhase::Ready),
        workflow_id: None,
        job_id: None,
        worker_identity: None,
        checkpoint: None,
        panel_count: 1,
    }
}

fn result() -> RemoteEnvironmentObservation {
    RemoteEnvironmentObservation {
        saved: summary(),
        observed_at_millis: 1_000,
        worker: None,
    }
}

fn pending(state: &mut ObservationState) -> mpsc::SyncSender<Result<RemoteEnvironmentObservation, ObservationError>> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingObservation {
        rx,
        expected: summary(),
        discard: false,
    });
    tx
}

#[test]
fn successful_observation_is_cached_with_absolute_timestamp_without_adoption() {
    let mut state = ObservationState::default();
    pending(&mut state).send(Ok(result())).expect("send");
    state.drain_result();
    let cached = state.last_success.as_ref().expect("cached success");
    assert_eq!(cached.checked_at, "1970-01-01T00:00:01Z");
    assert!(cached.lifecycle.contains("No exact worker found"));
    assert!(cached.resource_id.is_none());
    assert!(!state.is_pending());
    assert!(state.failure.is_none());
}

#[test]
fn all_lifecycles_are_distinct_from_workspace_readiness() {
    for lifecycle in [
        InteractiveWorkerLifecycle::Provisioning,
        InteractiveWorkerLifecycle::Ready,
        InteractiveWorkerLifecycle::Stopped,
        InteractiveWorkerLifecycle::Failed,
        InteractiveWorkerLifecycle::Deleting,
        InteractiveWorkerLifecycle::Unknown,
    ] {
        let mut observed = result();
        observed.worker = Some(ObservedRemoteWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::LocalDocker,
                workflow_id: CloudWorkflowId::new(),
                job_id: CloudJobId::new(),
                resource_id: "observed-resource-only".into(),
            },
            lifecycle,
        });
        let cached = CachedObservation::new(observed);
        assert!(cached.lifecycle.starts_with("Worker "));
        if lifecycle == InteractiveWorkerLifecycle::Ready {
            assert!(cached.lifecycle.contains("not workspace readiness"));
        }
        assert_eq!(cached.resource_id.as_deref(), Some("observed-resource-only"));
    }
    let mut invalid_time = result();
    invalid_time.observed_at_millis = i64::MAX;
    assert!(CachedObservation::new(invalid_time).checked_at.ends_with("Unix ms"));
}

#[test]
fn failed_repeat_and_disconnected_worker_preserve_timestamped_success() {
    let mut state = ObservationState {
        last_success: Some(CachedObservation::new(result())),
        ..Default::default()
    };
    pending(&mut state)
        .send(Err(ObservationError::StorageUnavailable))
        .expect("send");
    state.drain_result();
    assert!(
        state
            .failure
            .as_ref()
            .is_some_and(|error| error.contains("safely read"))
    );
    assert_eq!(
        state.last_success.as_ref().expect("old success").checked_at,
        "1970-01-01T00:00:01Z"
    );
    drop(pending(&mut state));
    state.drain_result();
    assert_eq!(
        state.failure.as_deref(),
        Some("The provider check failed to start or finish; you can retry now.")
    );
    assert!(!state.is_pending(), "a failed check releases the single-flight slot");
    assert!(state.last_success.is_some());
}

#[test]
fn repeated_invalidation_retains_single_flight_and_discards_late_results() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let mut state = ObservationState {
        last_success: Some(CachedObservation::new(result())),
        ..Default::default()
    };
    let tx = pending(&mut state);
    for _ in 0..100 {
        state.invalidate();
        state.start(&home, &RemoteProviderConfig::default(), &summary(), &Context::default());
        state.drain_result();
        assert!(state.is_pending());
        assert!(state.pending_label().contains("previous check"));
        assert!(state.last_success.is_none());
    }
    assert!(!home.cloud_workflow_store_path().exists());
    tx.send(Ok(result())).expect("send");
    state.drain_result();
    assert!(!state.is_pending());
    assert!(state.last_success.is_none());
    assert!(state.failure.is_none());
}

#[test]
fn unexpected_saved_snapshot_never_relabels_selection() {
    let mut state = ObservationState::default();
    let tx = pending(&mut state);
    let mut changed = result();
    changed.saved.revision += 1;
    tx.send(Ok(changed)).expect("send");
    state.drain_result();
    assert!(state.last_success.is_none());
    assert!(
        state
            .failure
            .as_ref()
            .is_some_and(|error| error.contains("selected saved environment"))
    );
}

fn view() -> RemoteEnvironments {
    let first = summary();
    let mut second = first.clone();
    second.workspace_local_id = "second-workspace".into();
    RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(InventoryPage {
            rows: vec![paint::InventoryRow::new(first), paint::InventoryRow::new(second)],
            next_cursor: None,
        }),
        observation: ObservationState {
            last_success: Some(CachedObservation::new(result())),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn selection_close_and_page_replacement_invalidate_pending_and_cached_results() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let mut state = view();
    let tx = pending(&mut state.observation);
    state.apply(InventoryAction::Select(0), &home, &Context::default());
    assert!(state.observation.last_success.is_some(), "same selection remains valid");
    state.apply(InventoryAction::Select(1), &home, &Context::default());
    assert!(state.observation.last_success.is_none());
    tx.send(Ok(result())).expect("send");
    state.observation.drain_result();
    assert!(state.observation.last_success.is_none());
    for close in [true, false] {
        let mut state = view();
        let tx = pending(&mut state.observation);
        if close {
            state.close();
        } else {
            state.accept_result(
                None,
                Ok(InventoryPage {
                    rows: Vec::new(),
                    next_cursor: None,
                }),
            );
        }
        tx.send(Ok(result())).expect("send");
        state.observation.drain_result();
        assert!(state.observation.last_success.is_none());
    }
}

#[test]
fn runtime_config_change_invalidates_only_provider_settings() {
    let (_fixture, mut app) = crate::app::test_support::test_app();
    app.remote_environments = view();
    let tx = pending(&mut app.remote_environments.observation);
    let mut config = app.template_config.clone();
    config.window.width += 1.0;
    app.apply_runtime_config(&config);
    assert!(app.remote_environments.observation.last_success.is_some());
    config
        .remote
        .local_docker
        .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///unused-fixture/docker.sock".into(),
        });
    app.apply_runtime_config(&config);
    tx.send(Ok(result())).expect("send");
    app.remote_environments.observation.drain_result();
    assert!(app.remote_environments.observation.last_success.is_none());
    assert!(app.remote_environments.observation.failure.is_none());
}

#[test]
fn closed_or_unselected_overview_never_starts_a_check() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let mut state = view();
    state.close();
    state.start_observation(&home, &RemoteProviderConfig::default(), &Context::default());
    state.open = true;
    state.selected = None;
    state.start_observation(&home, &RemoteProviderConfig::default(), &Context::default());
    assert!(!state.observation.is_pending());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn config_invalidation_after_empty_board_paint_requests_one_followup_frame() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_fixture, mut app) = crate::app::test_support::test_app();
    assert!(app.board.panels.is_empty());
    let ctx = Context::default();
    app.remote_environments = view();
    app.remote_environments.observation.repaint_context = Some(ctx.clone());
    pending(&mut app.remote_environments.observation)
        .send(Ok(result()))
        .expect("send");
    for frame in 0..30 {
        let mut input = raw_input([900.0, 680.0], None);
        input.time = Some(f64::from(frame) * 0.1);
        let _ = ctx
            .run_ui(input, |ui| {
                let _ = app.render_remote_environments(ui);
            })
            .discard_textures();
        if !ctx.has_requested_repaint() {
            break;
        }
    }
    assert!(!ctx.has_requested_repaint(), "settled status should not poll repaint");
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    ctx.set_request_repaint_callback(move |_| {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let mut config = app.template_config.clone();
    app.apply_runtime_config(&config);
    assert_eq!(requests.load(Ordering::Relaxed), 0);
    config
        .remote
        .local_docker
        .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///unused-fixture/docker.sock".into(),
        });
    app.apply_runtime_config(&config);
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert!(app.remote_environments.observation.last_success.is_none());
    app.remote_environments.invalidate_observation();
    assert_eq!(requests.load(Ordering::Relaxed), 1, "no passive invalidation loop");
}
