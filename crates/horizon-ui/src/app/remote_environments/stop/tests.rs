use super::super::{InventoryPage, RemoteEnvironments, paint as inventory_paint};
use super::*;
use horizon_core::cloud_run::{
    CloudJobId, CloudWorkflowId, WorkerLifetime, interactive_worker::InteractiveWorkerIdentity,
    local_docker::LocalDockerProfile,
};

fn summary() -> RemoteEnvironmentSummary {
    let workflow_id = CloudWorkflowId::new();
    let job_id = CloudJobId::new();
    RemoteEnvironmentSummary {
        workspace_local_id: "synthetic-workspace".into(),
        owning_session_id: "00000000-0000-4000-8000-000000000001".into(),
        revision: 1,
        repository: "example/project".into(),
        provider: CloudProvider::LocalDocker,
        profile: "development".into(),
        lifetime: WorkerLifetime::Persistent,
        generation: 1,
        saved_phase: Some(RemoteRuntimePhase::Reconciling),
        workflow_id: Some(workflow_id),
        job_id: Some(job_id),
        worker_identity: Some(InteractiveWorkerIdentity {
            provider: CloudProvider::LocalDocker,
            workflow_id,
            job_id,
            resource_id: "a".repeat(64),
        }),
        checkpoint: None,
        panel_count: 1,
    }
}

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: vec![LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///unused-stop-fixture/docker.sock".into(),
        }],
    }
}

fn completed(expected: &RemoteEnvironmentSummary) -> RemoteEnvironmentSummary {
    let mut saved = expected.clone();
    saved.revision += 2;
    saved.saved_phase = Some(RemoteRuntimePhase::Stopped {
        requested_at_millis: 1,
        observed_at_millis: 2,
    });
    saved
}

fn pending(
    state: &mut StopState,
    expected: &RemoteEnvironmentSummary,
) -> mpsc::SyncSender<Result<RemoteEnvironmentSummary, StopError>> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingStop {
        rx,
        expected: expected.clone(),
        discard: false,
    });
    tx
}

fn page(expected: &RemoteEnvironmentSummary) -> InventoryPage {
    let mut other = expected.clone();
    other.workspace_local_id = "second-workspace".into();
    InventoryPage {
        rows: vec![
            inventory_paint::InventoryRow::new(expected.clone()),
            inventory_paint::InventoryRow::new(other),
        ],
        next_cursor: None,
    }
}

fn view(expected: &RemoteEnvironmentSummary) -> RemoteEnvironments {
    RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(page(expected)),
        ..Default::default()
    }
}

#[test]
fn confirmation_is_inert_and_exact_selection_and_config_are_required() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = StopState::default();
    assert!(!state.start(&home, &config(), &expected, &ctx));
    state.prepare(&expected, &config(), &ctx);
    assert!(state.confirmation.is_some());
    assert!(!home.cloud_workflow_store_path().exists());
    state.cancel_confirmation();
    assert!(!state.start(&home, &config(), &expected, &ctx));
    state.prepare(&expected, &config(), &ctx);
    let mut changed = expected.clone();
    changed.revision += 1;
    assert!(!state.start(&home, &config(), &changed, &ctx));
    state.prepare(&expected, &config(), &ctx);
    assert!(!state.start(&home, &RemoteProviderConfig::default(), &expected, &ctx));
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        changed.provider = provider;
        state.prepare(&changed, &config(), &ctx);
        assert!(state.confirmation.is_none());
    }
    changed = expected;
    changed.worker_identity = None;
    state.prepare(&changed, &config(), &ctx);
    assert!(state.confirmation.is_none());
    assert!(!state.is_pending());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn cancel_selection_page_and_close_clear_only_unconfirmed_intent() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    for action in [
        InventoryAction::CancelStop,
        InventoryAction::Select(1),
        InventoryAction::Close,
    ] {
        let mut state = view(&expected);
        state.stop_action(InventoryAction::RequestStop, &home, &config(), &ctx);
        assert!(state.stop.confirmation.is_some());
        state.apply(action, &home, &ctx);
        state.stop_action(InventoryAction::ConfirmStop, &home, &config(), &ctx);
        assert!(state.stop.confirmation.is_none());
        assert!(!state.stop.is_pending());
    }
    let mut state = view(&expected);
    state.stop.prepare(&expected, &config(), &ctx);
    state.accept_result(None, Ok(page(&expected)));
    assert!(state.stop.confirmation.is_none());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn confirmed_operation_stays_single_flight_through_close_and_invalidation() {
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = view(&expected);
    let tx = pending(&mut state.stop, &expected);
    for _ in 0..100 {
        state.close();
        state.open = true;
        state.stop.prepare(&expected, &config(), &ctx);
        assert!(!state.stop.start(&home, &config(), &expected, &ctx));
        state.start_observation(&home, &config(), &ctx);
        assert!(!state.observation.is_pending());
        assert!(!state.stop.drain_result());
        assert!(state.stop.is_pending());
    }
    tx.send(Ok(completed(&expected))).expect("send");
    assert!(state.stop.drain_result());
    assert!(
        state.stop.notice.is_none(),
        "closed selection discards presentation only"
    );
    assert!(!state.stop.is_pending());
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn result_remains_readable_after_saved_page_refresh_but_is_bound_to_exact_target() {
    let expected = summary();
    for succeed in [true, false] {
        let mut state = view(&expected);
        let result = if succeed {
            Ok(completed(&expected))
        } else {
            Err(StopError::StorageUnavailable)
        };
        pending(&mut state.stop, &expected).send(result).expect("send");
        assert!(state.stop.drain_result());
        state.accept_result(None, Ok(page(&completed(&expected))));
        let notice = state.stop.notice.as_ref().expect("last explicit result");
        assert_eq!(notice.succeeded, succeed);
        assert!(same_target(&notice.expected, &completed(&expected)));
        let mut another = expected.clone();
        another.worker_identity.as_mut().expect("identity").resource_id = "b".repeat(64);
        assert!(!same_target(&notice.expected, &another));
        another = expected.clone();
        another.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
        assert!(!same_target(&notice.expected, &another));
        assert!(!notice.message.is_empty());
    }
}

#[test]
fn manual_page_actions_discard_notices_but_retain_the_confirmed_operation() {
    use super::super::{LoadError, LoadFailure};
    let fixture = tempfile::tempdir().expect("fixture");
    let expected = summary();
    let ctx = Context::default();
    for (index, action) in [
        InventoryAction::Refresh,
        InventoryAction::First,
        InventoryAction::Next,
        InventoryAction::Retry,
    ]
    .into_iter()
    .enumerate()
    {
        let home = HorizonHome::from_root(fixture.path().join(format!("navigation-{index}")));
        let mut state = view(&expected);
        state.page.as_mut().expect("page").next_cursor = Some("next-workspace".into());
        state.page_cursor = Some("previous-workspace".into());
        state.failure = Some(LoadFailure {
            error: LoadError::ReadPage,
            cursor: None,
        });
        state.stop.notice = Some(StopNotice::new(expected.clone(), Ok(completed(&expected))));
        let tx = pending(&mut state.stop, &expected);
        state.apply(action, &home, &ctx);
        assert!(state.stop.notice.is_none());
        assert!(state.stop.is_pending());
        let load = state.pending.take().expect("explicit page read");
        assert!(load.rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
        tx.send(Ok(completed(&expected))).expect("send");
        assert!(state.stop.drain_result());
        assert!(
            state.stop.notice.is_none(),
            "navigation discards the late presentation, not the operation"
        );
    }
}

#[test]
fn unexpected_completion_and_disconnected_worker_never_claim_success() {
    let expected = summary();
    let mut changes = vec![completed(&expected); 4];
    changes[0].workspace_local_id = "foreign-workspace".into();
    changes[1].generation += 1;
    changes[2].saved_phase = Some(RemoteRuntimePhase::Ready);
    changes[3].revision = 0;
    for changed in changes {
        let notice = StopNotice::new(expected.clone(), Ok(changed));
        assert!(!notice.succeeded);
        assert!(notice.message.contains("does not match"));
    }
    let mut state = StopState::default();
    drop(pending(&mut state, &expected));
    assert!(state.drain_result());
    let notice = state.notice.as_ref().expect("worker failure");
    assert!(!notice.succeeded);
    assert!(notice.message.contains("Refresh saved inventory"));
}

#[test]
fn provider_reload_invalidates_confirmation_but_unrelated_visual_reload_does_not() {
    let (_fixture, mut app) = crate::app::test_support::test_app();
    let expected = summary();
    app.remote_environments = view(&expected);
    app.remote_environments
        .stop
        .prepare(&expected, &app.template_config.remote, &Context::default());
    let mut updated = app.template_config.clone();
    updated.window.width += 1.0;
    app.apply_runtime_config(&updated);
    assert!(app.remote_environments.stop.confirmation.is_some());
    let tx = pending(&mut app.remote_environments.stop, &expected);
    updated.remote = config();
    app.apply_runtime_config(&updated);
    assert!(app.remote_environments.stop.confirmation.is_none());
    assert!(app.remote_environments.stop.is_pending());
    tx.send(Ok(completed(&expected))).expect("send");
    assert!(app.remote_environments.stop.drain_result());
    assert!(app.remote_environments.stop.notice.is_none());
}

#[test]
fn completion_queues_saved_refresh_behind_inventory_read_and_closed_view_does_not_load() {
    use super::super::PendingLoad;
    let fixture = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(fixture.path().join("unused"));
    let expected = summary();
    let ctx = Context::default();
    let mut state = view(&expected);
    let (inventory_tx, inventory_rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingLoad {
        rx: inventory_rx,
        cursor: None,
        discard: false,
    });
    pending(&mut state.stop, &expected)
        .send(Ok(completed(&expected)))
        .expect("send");
    state.drain_stop(&home, &ctx);
    assert!(state.refresh_after_stop);
    assert!(state.stop.notice.as_ref().is_some_and(|notice| notice.succeeded));
    assert!(!home.cloud_workflow_store_path().exists());
    inventory_tx.send(Ok(page(&expected))).expect("page");
    state.drain_result();
    state.drain_stop(&home, &ctx);
    assert!(!state.refresh_after_stop);
    let refresh = state.pending.take().expect("queued refresh started");
    assert!(refresh.rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok());
    assert!(state.stop.notice.is_some());
    let untouched = HorizonHome::from_root(fixture.path().join("closed"));
    let tx = pending(&mut state.stop, &expected);
    state.close();
    tx.send(Ok(completed(&expected))).expect("send");
    state.drain_stop(&untouched, &ctx);
    assert!(!state.refresh_after_stop);
    assert!(!untouched.cloud_workflow_store_path().exists());
}

#[test]
fn confirmation_never_consumes_enter_as_an_implicit_stop() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    let ctx = Context::default();
    let expected = summary();
    let mut state = StopState::default();
    state.prepare(&expected, &config(), &ctx);
    for enter in [false, true] {
        let mut input = raw_input([1100.0, 780.0], None);
        if enter {
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }
        let mut action = InventoryAction::None;
        let _ = ctx
            .run_ui(input, |ui| show(ui, &state, &expected, true, &mut action))
            .discard_textures();
        assert!(matches!(action, InventoryAction::None));
        assert!(state.confirmation.is_some());
        assert!(!state.is_pending());
    }
}
