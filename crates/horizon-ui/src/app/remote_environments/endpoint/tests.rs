use super::super::{InventoryPage, paint as inventory_paint};
use super::*;
#[cfg(target_os = "linux")]
use crate::{app::test_support::raw_input, test_egui::DiscardTextures};
use horizon_core::cloud_run::{CloudJobId, CloudWorkflowId, interactive_worker::InteractiveWorkerIdentity};

struct Fixture {
    temp: tempfile::TempDir,
    scope: Scope,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("synthetic home");
        let workflow_id = CloudWorkflowId::new();
        let job_id = CloudJobId::new();
        let expected = RemoteEnvironmentSummary {
            workspace_local_id: "endpoint-fixture".into(),
            owning_session_id: "synthetic-owner".into(),
            revision: 4,
            repository: "example/project".into(),
            provider: CloudProvider::RunPod,
            profile: "fixture".into(),
            lifetime: WorkerLifetime::Persistent,
            generation: 1,
            saved_phase: Some(RemoteRuntimePhase::Ready),
            workflow_id: Some(workflow_id),
            job_id: Some(job_id),
            worker_identity: Some(InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id,
                job_id,
                resource_id: "synthetic-retained-pod".into(),
            }),
            checkpoint: None,
            panel_count: 1,
        };
        Self {
            scope: Scope {
                home: HorizonHome::from_root(temp.path().join("home")),
                config: RemoteProviderConfig::default(),
                expected,
            },
            temp,
        }
    }
    fn request(&self, state: &mut EndpointState, action: Action) -> Option<Scope> {
        state.request(action, &self.scope.home, &self.scope.config, &self.scope.expected)
    }
    fn pending(&self, state: &mut EndpointState) -> mpsc::SyncSender<Result<ConfiguredEndpointRefresh, Error>> {
        let (tx, rx) = mpsc::sync_channel(1);
        state.pending = Some(Pending {
            scope: self.scope.clone(),
            rx,
            discard: false,
        });
        tx
    }
    fn view(&self) -> RemoteEnvironments {
        RemoteEnvironments {
            open: true,
            selected: Some(0),
            page: Some(InventoryPage {
                rows: vec![inventory_paint::InventoryRow::new(self.scope.expected.clone())],
                next_cursor: None,
            }),
            ..Default::default()
        }
    }
}

#[test]
fn eligibility_is_coarse_platform_phase_and_owned_identity_only() {
    let fixture = Fixture::new();
    assert!(fixture.temp.path().exists());
    for phase in [
        RemoteRuntimePhase::Ready,
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
        RemoteRuntimePhase::Starting { requested_at_millis: 1 },
    ] {
        let mut summary = fixture.scope.expected.clone();
        summary.saved_phase = Some(phase);
        assert_eq!(supported(&summary), cfg!(target_os = "linux"));
    }
    for fault in 0..10 {
        let mut summary = fixture.scope.expected.clone();
        match fault {
            0 => summary.provider = CloudProvider::Azure,
            1 => summary.lifetime = WorkerLifetime::TimeLimited { seconds: 60 },
            2 => summary.worker_identity = None,
            3 => summary.workflow_id = None,
            4 => summary.job_id = None,
            5 => summary.worker_identity.as_mut().expect("worker").resource_id.clear(),
            6 => summary.saved_phase = None,
            7 => summary.saved_phase = Some(RemoteRuntimePhase::Stopping { requested_at_millis: 1 }),
            8 => summary.saved_phase = Some(RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 }),
            _ => {
                summary.saved_phase = Some(RemoteRuntimePhase::Deleted {
                    requested_at_millis: 1,
                    observed_at_millis: 2,
                });
            }
        }
        assert!(!supported(&summary));
    }
    assert!(!fixture.scope.home.root().exists());
}

#[test]
#[cfg(target_os = "linux")]
fn confirmation_is_consumed_requires_consent_and_binds_home_config_and_selection() {
    let fixture = Fixture::new();
    for fault in 0..4 {
        let mut state = EndpointState::default();
        assert!(fixture.request(&mut state, Action::Request).is_none());
        assert!(fixture.request(&mut state, Action::Confirm).is_none());
        fixture.request(&mut state, Action::Request);
        state.confirmation.as_mut().expect("confirmation").acknowledged = true;
        let mut changed = fixture.scope.clone();
        match fault {
            0 => changed.home = HorizonHome::from_root(fixture.temp.path().join("other")),
            1 => changed
                .config
                .local_docker
                .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
                    name: "other".into(),
                    docker_host: "unix:///synthetic.sock".into(),
                }),
            2 => changed.expected.revision += 1,
            _ => {
                fixture.request(&mut state, Action::Cancel);
            }
        }
        assert!(
            state
                .request(Action::Confirm, &changed.home, &changed.config, &changed.expected)
                .is_none()
        );
    }
    let mut state = EndpointState::default();
    fixture.request(&mut state, Action::Request);
    state.confirmation.as_mut().expect("confirmation").acknowledged = true;
    assert!(fixture.request(&mut state, Action::Confirm).is_some());
    assert!(fixture.request(&mut state, Action::Confirm).is_none());
    assert!(!fixture.scope.home.root().exists());
}

#[test]
fn actual_executor_refuses_missing_store_without_creating_home() {
    let fixture = Fixture::new();
    assert!(matches!(execute(&fixture.scope), Err(Error::Storage)));
    assert!(!fixture.scope.home.root().exists());
}

#[test]
fn pending_blocks_synthetic_dispatch_and_defers_inventory_until_completion() {
    let fixture = Fixture::new();
    let mut view = fixture.view();
    let tx = fixture.pending(&mut view.endpoint);
    for action in [
        InventoryAction::Refresh,
        InventoryAction::First,
        InventoryAction::Next,
        InventoryAction::Observe,
        InventoryAction::RequestStop,
        InventoryAction::ConfirmStop,
        InventoryAction::RequestStart,
        InventoryAction::ConfirmStart,
        InventoryAction::CheckStop,
        InventoryAction::PrepareRepository,
        InventoryAction::ConfirmRepository,
        InventoryAction::InspectRepository,
        InventoryAction::ListReconnectViews,
        InventoryAction::ListReopenPanels,
        InventoryAction::ConfirmTaskStart,
        InventoryAction::WorkspaceSetup(super::super::setup::Action::New),
        InventoryAction::Delete(super::super::delete::Action::Request),
        InventoryAction::Delete(super::super::delete::Action::Confirm),
        InventoryAction::Endpoint(Action::Request),
        InventoryAction::Endpoint(Action::Confirm),
    ] {
        assert!(matches!(view.guard_endpoint_action(action), InventoryAction::None));
    }
    assert!(matches!(
        view.guard_endpoint_action(InventoryAction::Close),
        InventoryAction::Close
    ));
    assert!(matches!(
        view.guard_endpoint_action(InventoryAction::Select(0)),
        InventoryAction::Select(0)
    ));
    view.start_load(&fixture.scope.home, &Context::default(), None);
    assert!(view.pending.is_none() && view.refresh_when_idle);
    assert!(fixture.request(&mut view.endpoint, Action::Confirm).is_none());
    tx.send(Err(Error::Worker)).expect("completion");
    view.drain_endpoint(&fixture.scope.home, &fixture.scope.config);
    assert!(!view.endpoint.is_pending() && view.refresh_when_idle);
    assert!(view.endpoint.notice.is_some());
    view.refresh_when_idle = false;
    view.drain_endpoint(&fixture.scope.home, &fixture.scope.config);
    assert!(!view.refresh_when_idle);
}

#[test]
fn other_pending_work_refuses_endpoint_and_invalidation_keeps_receiver() {
    let fixture = Fixture::new();
    let mut view = fixture.view();
    let (_tx, rx) = mpsc::sync_channel(1);
    view.pending = Some(super::super::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    assert!(matches!(
        view.guard_endpoint_action(InventoryAction::Endpoint(Action::Request)),
        InventoryAction::None
    ));
    view.endpoint_action(
        InventoryAction::Endpoint(Action::Request),
        &fixture.scope.home,
        &fixture.scope.config,
        &Context::default(),
    );
    assert!(view.endpoint.confirmation.is_none());
    view.pending = None;
    let tx = fixture.pending(&mut view.endpoint);
    view.close();
    assert!(view.endpoint.is_pending());
    tx.send(Err(Error::Worker)).expect("owned completion");
    view.drain_endpoint(&fixture.scope.home, &fixture.scope.config);
    assert!(!view.endpoint.is_pending() && view.endpoint.notice.is_none() && !view.refresh_when_idle);
    let tx = fixture.pending(&mut view.endpoint);
    view.open(&fixture.scope.home, &Context::default());
    assert!(view.pending.is_none() && view.endpoint.is_pending() && view.refresh_when_idle);
    tx.send(Err(Error::Worker)).expect("discarded completion");
    view.drain_endpoint(&fixture.scope.home, &fixture.scope.config);
    assert!(view.endpoint.notice.is_none());
    assert!(!fixture.scope.home.root().exists());
}

#[test]
fn lost_reply_and_stale_scope_drain_without_automatic_retry() {
    let fixture = Fixture::new();
    let mut state = EndpointState::default();
    drop(fixture.pending(&mut state));
    assert!(state.drain());
    assert!(
        state
            .notice
            .as_ref()
            .expect("notice")
            .message
            .contains("No retry is automatic")
    );
    let tx = fixture.pending(&mut state);
    let mut changed = fixture.scope.expected.clone();
    changed.revision += 1;
    state.sync(&fixture.scope.home, &fixture.scope.config, Some(&changed));
    assert!(state.pending.as_ref().expect("still owned").discard);
    tx.send(Err(Error::Worker)).expect("completion");
    assert!(state.drain());
    assert!(state.notice.is_none() && !state.drain());
}

#[test]
#[cfg(target_os = "linux")]
fn result_preserves_phase_identity_and_accepts_only_noop_or_one_revision() {
    let fixture = Fixture::new();
    for delta in [0, 1] {
        let mut saved = fixture.scope.expected.clone();
        saved.revision += delta;
        let notice = Notice::finish(
            fixture.scope.clone(),
            Ok(ConfiguredEndpointRefresh { saved: saved.clone() }),
        );
        assert_eq!(notice.saved, Some(saved.clone()));
        let mut state = EndpointState {
            notice: Some(notice),
            ..Default::default()
        };
        state.sync(&fixture.scope.home, &fixture.scope.config, Some(&saved));
        assert!(state.notice.is_some());
    }
    for fault in 0..5 {
        let mut saved = fixture.scope.expected.clone();
        match fault {
            0 => saved.revision += 2,
            1 => saved.revision -= 1,
            2 => saved.saved_phase = Some(RemoteRuntimePhase::Failed),
            3 => saved.profile = "different".into(),
            _ => saved.worker_identity.as_mut().expect("worker").resource_id = "replacement".into(),
        }
        assert!(
            Notice::finish(fixture.scope.clone(), Ok(ConfiguredEndpointRefresh { saved }))
                .saved
                .is_none()
        );
    }
}

#[test]
#[cfg(target_os = "linux")]
fn render_is_passive_and_keyboard_does_not_submit_acknowledged_confirmation() {
    fn text(shape: &egui::Shape, output: &mut String) {
        match shape {
            egui::Shape::Text(value) => output.push_str(value.galley.text()),
            egui::Shape::Vec(values) => values.iter().for_each(|value| text(value, output)),
            _ => {}
        }
    }
    let fixture = Fixture::new();
    for size in [[1200.0, 900.0], [800.0, 600.0]] {
        let ctx = Context::default();
        let mut state = EndpointState::default();
        fixture.request(&mut state, Action::Request);
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input(size, None), |ui| {
            state.show(ui, &fixture.scope.expected, true, &mut action);
        });
        let mut visible = String::new();
        for shape in &output.shapes {
            text(&shape.shape, &mut visible);
        }
        for required in [
            "endpoint-fixture",
            "synthetic-retained-pod",
            "/usr/bin/true",
            "no key discovery or rotation",
            "no panel reconnect or task replay",
            "Start intent is preserved",
            "Cancel connection refresh",
        ] {
            assert!(visible.contains(required), "missing {required}");
        }
        let _ = output.discard_textures();
        let button = ctx
            .data(|data| data.get_temp::<(egui::Id, bool)>(egui::Id::new("endpoint-confirm")))
            .expect("confirm button");
        assert!(!button.1 && matches!(action, InventoryAction::None));
        state.confirmation.as_mut().expect("confirmation").acknowledged = true;
        ctx.memory_mut(|memory| memory.request_focus(button.0));
        for key in [egui::Key::Enter, egui::Key::Space] {
            let mut input = raw_input(size, None);
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
            let _ = ctx
                .run_ui(input, |ui| state.show(ui, &fixture.scope.expected, true, &mut action))
                .discard_textures();
            assert!(matches!(action, InventoryAction::None));
        }
        assert!(!state.is_pending() && !fixture.scope.home.root().exists());
        let mut view = fixture.view();
        let _tx = fixture.pending(&mut view.endpoint);
        let _ = ctx
            .run_ui(raw_input([1200.0, 1400.0], None), |ctx| {
                inventory_paint::show(ctx, &mut view);
            })
            .discard_textures();
        for label in ["Delete environment…", "Check saved Delete", "Retry Delete…"] {
            assert!(
                !ctx.data(|data| data.get_temp::<(egui::Rect, bool)>(egui::Id::new(label)))
                    .expect("delete button")
                    .1
            );
        }
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
            Some(false)
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<bool>(egui::Id::new("endpoint-request"))),
            Some(false)
        );
    }
}
