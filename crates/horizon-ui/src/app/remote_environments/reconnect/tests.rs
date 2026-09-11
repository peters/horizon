use super::super::{InventoryPage, RemoteEnvironments, paint::InventoryRow};
use super::*;
use crate::app::test_support::{raw_input, test_app};
use crate::test_egui::DiscardTextures;
use horizon_core::{
    PanelKind, PanelState, RemoteWorkspaceReference, RuntimeState, WorkspaceState,
    cloud_run::{CloudProvider, WorkerLifetime},
};

fn summary(owner: &str) -> RemoteEnvironmentSummary {
    RemoteEnvironmentSummary {
        workspace_local_id: "retained-workspace".into(),
        owning_session_id: owner.into(),
        revision: 1,
        repository: "example/project".into(),
        provider: CloudProvider::LocalDocker,
        profile: "development".into(),
        lifetime: WorkerLifetime::Persistent,
        generation: 1,
        saved_phase: None,
        workflow_id: None,
        job_id: None,
        worker_identity: None,
        checkpoint: None,
        panel_count: 3,
    }
}

fn runtime(owner: &str) -> RuntimeState {
    let reference = RemoteWorkspaceReference::new(owner.into(), "retained-workspace".into()).expect("reference");
    RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "visual-workspace".into(),
            name: "Retained work".into(),
            remote_workspace: Some(reference.clone()),
            panels: (0..3)
                .map(|index| PanelState {
                    local_id: format!("task-{index}"),
                    name: format!("Task {index}"),
                    kind: PanelKind::Ssh,
                    remote_workspace: Some(reference.clone()),
                    ..PanelState::default()
                })
                .collect(),
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}

fn inventory(expected: &RemoteEnvironmentSummary) -> RemoteEnvironments {
    let mut other = expected.clone();
    other.workspace_local_id = "other-workspace".into();
    RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(InventoryPage {
            rows: vec![InventoryRow::new(expected.clone()), InventoryRow::new(other)],
            next_cursor: None,
        }),
        ..Default::default()
    }
}

fn pending(
    state: &mut ReconnectState,
    expected: &RemoteEnvironmentSummary,
) -> mpsc::SyncSender<Result<PreparedRemotePanelHandoff, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.views = Some(vec![views::View {
        id: PanelId(1),
        label: "Cached view".into(),
    }]);
    state.pending = Some(PendingConnection {
        rx,
        expected: expected.clone(),
        owner: expected.owning_session_id.clone(),
        config: RemoteProviderConfig::default(),
        target: PanelId(1),
        discard: false,
    });
    tx
}

#[test]
fn runpod_existing_views_use_same_owner_admission_and_discard_changed_profile_results() {
    let owner = "00000000-0000-4000-8000-000000000001";
    let mut expected = summary(owner);
    expected.provider = CloudProvider::RunPod;
    let mut board = Board::from_runtime_state(&runtime(owner)).expect("inert saved views");
    for panel in &mut board.panels {
        assert!(panel.wait_for_shutdown(std::time::Duration::from_secs(2)));
        panel.process_output();
    }
    let listed = views::list(&board, Some(&expected), Some(owner)).expect("RunPod saved views");
    assert_eq!(listed.len(), 3);
    assert!(views::list(&board, Some(&expected), Some("foreign-owner")).is_err());
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("unused"));
    let mut state = ReconnectState::default();
    let tx = pending(&mut state, &expected);
    let mut config = RemoteProviderConfig::default();
    config.runpod.push(
        serde_json::from_value(serde_json::json!({
            "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
            "ports":["22/tcp"], "volume_gib":0
        }))
        .expect("non-secret profile"),
    );
    state.pending.as_mut().expect("pending").config = config.clone();
    config.runpod[0].data_center_id = Some("EU-RO-1".into());
    let client = ClientContext {
        home: &home,
        config: &config,
        selected: Some(&expected),
        owner: Some(owner),
    };
    tx.send(Err("stale RunPod result".into())).expect("completion");
    assert_eq!(state.drain(&client, &mut board, &Context::default()), None);
    assert!(state.pending.is_none());
    assert!(state.notice.is_none());
    assert_eq!(board.panels.len(), 3);
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn cached_list_requires_actual_owner_and_revalidates_current_target_before_io() {
    let owner = "00000000-0000-4000-8000-000000000001";
    let expected = summary(owner);
    let mut board = Board::from_runtime_state(&runtime(owner)).expect("inert views");
    for panel in &mut board.panels {
        assert!(panel.wait_for_shutdown(std::time::Duration::from_secs(2)));
        panel.process_output();
    }
    assert!(views::list(&board, Some(&expected), None).is_err());
    assert!(views::list(&board, Some(&expected), Some("copied-owner")).is_err());
    assert_eq!(
        views::list(&board, Some(&expected), Some(owner)).expect("views").len(),
        3
    );
    let ctx = Context::default();
    let _ = ctx.run_ui(raw_input([900.0, 700.0], None), |_| {}).discard_textures();
    let id = board.panels[0].id;
    board.panels[0].resize_immediately(31, 99, 10, 18);
    let request = views::request(&board, &expected, owner, id, &ctx).expect("request");
    assert_eq!(request.local_id, "task-0");
    assert_eq!((request.terminal.rows, request.terminal.cols), (31, 99));
    assert!(views::request(&board, &expected, "copied-owner", id, &ctx).is_err());
    board.panels[0].kind = PanelKind::Shell;
    assert!(views::request(&board, &expected, owner, id, &ctx).is_err());
    board.close_panel(id);
    assert!(views::request(&board, &expected, owner, id, &ctx).is_err());
}

#[test]
fn invalidation_retains_single_flight_until_completion_and_allows_explicit_retry() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("unused"));
    let expected = summary("00000000-0000-4000-8000-000000000001");
    let config = RemoteProviderConfig::default();
    let client = ClientContext {
        home: &home,
        config: &config,
        selected: Some(&expected),
        owner: Some(&expected.owning_session_id),
    };
    let ctx = Context::default();
    let mut board = Board::new();
    let mut state = ReconnectState::default();
    let tx = pending(&mut state, &expected);
    state.invalidate();
    state.list(&client, &board, &ctx);
    state.start(&client, &board, PanelId(1), &ctx);
    assert_eq!(state.drain(&client, &mut board, &ctx), None);
    assert!(state.pending.as_ref().expect("retained slot").discard);
    assert!(state.views.is_none());
    tx.send(Err("discarded-result".into())).expect("send");
    assert_eq!(state.drain(&client, &mut board, &ctx), None);
    assert!(state.pending.is_none());
    assert!(state.notice.is_none());
    state.list(&client, &board, &ctx);
    assert!(state.views.as_ref().expect("explicit retry").is_empty());
    drop(pending(&mut state, &expected));
    assert_eq!(state.drain(&client, &mut board, &ctx), None);
    assert_eq!(state.notice.as_deref(), Some(worker_failure()));
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn completion_rechecks_actual_owner_selection_and_config_even_without_invalidation() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("unused"));
    let expected = summary("00000000-0000-4000-8000-000000000001");
    for fault in 0..4 {
        let mut state = ReconnectState::default();
        let tx = pending(&mut state, &expected);
        let mut selected = expected.clone();
        let mut config = RemoteProviderConfig::default();
        if fault == 1 {
            selected.revision += 1;
        }
        if fault == 2 {
            config
                .local_docker
                .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
                    name: "changed".into(),
                    docker_host: "unix:///unused/socket".into(),
                });
        }
        let client = ClientContext {
            home: &home,
            config: &config,
            selected: (fault != 3).then_some(&selected),
            owner: Some(if fault == 0 {
                "other-owner"
            } else {
                &expected.owning_session_id
            }),
        };
        tx.send(Err("stale-result".into())).expect("send");
        assert_eq!(state.drain(&client, &mut Board::new(), &Context::default()), None);
        assert!(state.notice.is_none());
        assert!(state.pending.is_none());
    }
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn navigation_refresh_stop_and_provider_reload_invalidate_pending_results() {
    for action in [
        InventoryAction::Close,
        InventoryAction::Select(1),
        InventoryAction::Refresh,
        InventoryAction::RequestStop,
        InventoryAction::ListReopenPanels,
        InventoryAction::ReopenView(0),
    ] {
        let (_temp, mut app) = test_app();
        let expected = summary("00000000-0000-4000-8000-000000000001");
        app.remote_environments = inventory(&expected);
        let tx = pending(&mut app.remote_environments.reconnect, &expected);
        tx.send(Err("discarded-result".into())).expect("queued send");
        let ctx = Context::default();
        if matches!(action, InventoryAction::RequestStop) {
            app.remote_environments
                .stop_action(action, app.session_store.home(), &app.template_config.remote, &ctx);
        }
        app.remote_environments.apply(action, app.session_store.home(), &ctx);
        assert!(
            app.remote_environments
                .reconnect
                .pending
                .as_ref()
                .expect("slot")
                .discard
        );
        app.remote_reconnect_action(InventoryAction::None, &ctx);
        assert!(app.remote_environments.reconnect.pending.is_none());
        assert!(app.remote_environments.reconnect.notice.is_none());
    }
    let (_temp, mut app) = test_app();
    let expected = summary("00000000-0000-4000-8000-000000000001");
    app.remote_environments = inventory(&expected);
    let _tx = pending(&mut app.remote_environments.reconnect, &expected);
    let mut config = app.template_config.clone();
    config.window.width += 1.0;
    app.apply_runtime_config(&config);
    assert!(
        !app.remote_environments
            .reconnect
            .pending
            .as_ref()
            .expect("slot")
            .discard
    );
    config
        .remote
        .local_docker
        .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
            name: "changed".into(),
            docker_host: "unix:///unused/socket".into(),
        });
    app.apply_runtime_config(&config);
    assert!(
        app.remote_environments
            .reconnect
            .pending
            .as_ref()
            .expect("slot")
            .discard
    );
}

#[test]
fn same_owner_restoration_and_session_switch_discard_reused_panel_ids() {
    let (_temp, mut app) = test_app();
    let mut session = app
        .session_store
        .create_session_from_runtime(RuntimeState::default())
        .expect("session");
    session.runtime_state = runtime(&session.session_id);
    app.activate_persistent_session(&session);
    let expected = summary(&session.session_id);
    for switch in [false, true] {
        let previous_id = app.board.panels[0].id;
        app.remote_environments = inventory(&expected);
        let tx = pending(&mut app.remote_environments.reconnect, &expected);
        if switch {
            switch_through_session_manager(&mut app);
        } else {
            app.activate_persistent_session(&session);
        }
        assert!(
            app.remote_environments
                .reconnect
                .pending
                .as_ref()
                .expect("slot")
                .discard
        );
        assert!(app.remote_environments.reconnect.views.is_none());
        if !switch {
            assert_eq!(app.board.panels[0].id, previous_id);
        }
        tx.send(Err("old-board-result".into())).expect("send");
        app.remote_reconnect_action(InventoryAction::None, &Context::default());
        assert!(app.remote_environments.reconnect.notice.is_none());
    }
}

fn switch_through_session_manager(app: &mut crate::app::HorizonApp) {
    let ctx = Context::default();
    app.root_viewport_stabilizer = None;
    app.toggle_session_manager();
    let mut output = ctx
        .run_ui(raw_input([1000.0, 900.0], None), |ui| app.render_session_manager(ui))
        .discard_textures();
    for _ in 0..2 {
        output = ctx
            .run_ui(raw_input([1000.0, 900.0], None), |ui| app.render_session_manager(ui))
            .discard_textures();
    }
    let position = output
        .shapes
        .iter()
        .find_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape
                && text.galley.job.text == "Open New Session"
            {
                Some(text.pos + text.galley.rect.center().to_vec2())
            } else {
                None
            }
        })
        .expect("session manager action");
    for pressed in [true, false] {
        let mut input = raw_input([1000.0, 900.0], None);
        input.events.push(egui::Event::PointerMoved(position));
        input.events.push(egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx
            .run_ui(input, |ui| app.render_session_manager(ui))
            .discard_textures();
    }
}

#[test]
fn dismissal_paints_pending_state_before_draining_a_queued_result() {
    let (_temp, mut app) = test_app();
    let expected = summary("00000000-0000-4000-8000-000000000001");
    app.remote_environments = inventory(&expected);
    let ctx = Context::default();
    let _ = ctx
        .run_ui(raw_input([1000.0, 900.0], None), |ui| {
            app.render_remote_environments(ui);
        })
        .discard_textures();
    pending(&mut app.remote_environments.reconnect, &expected)
        .send(Err("queued-result".into()))
        .expect("send");
    let mut input = raw_input([1000.0, 900.0], None);
    input.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(input, |ui| {
            app.render_remote_environments(ui);
        })
        .discard_textures();
    assert_eq!(
        ctx.data(|data| data.get_temp::<bool>(egui::Id::new("reconnect-painted-pending-test"))),
        Some(true)
    );
    assert!(!app.remote_environments.open);
    assert!(app.remote_environments.reconnect.pending.is_none());
    assert!(app.remote_environments.reconnect.notice.is_none());
}
