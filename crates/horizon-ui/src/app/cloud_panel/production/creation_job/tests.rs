use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::CloudConfig;

fn pending(app: &mut HorizonApp) -> std::sync::mpsc::Sender<cloud_runtime::Result<Resolved>> {
    let workspace = app.board.ensure_workspace();
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let (sender, receiver) = channel();
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.pending_creation = Some(Pending {
        receiver,
        cancel: CancelOnDrop(cloud_runtime::Cancellation::default()),
        session: app.active_session.as_ref().map(|session| session.session_id.clone()),
        workspace: app.board.workspace(workspace).unwrap().local_id.clone(),
        title: "Captured title".into(),
        launch: CloudLaunch {
            deployment_started: false,
            id: cloud_runtime::new_id(),
            revision: String::new(),
            profile_name: "dev".into(),
            profile: config.profiles["dev"].clone(),
        },
    });
    sender
}

fn resolved(path: &std::path::Path) -> Resolved {
    Resolved {
        repository: path.into(),
        revision: "a".repeat(40),
    }
}

#[test]
fn pending_resolution_is_nonblocking_and_cancel_discards_late_results() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.production.pending_creation.is_some());
    assert!(app.cloud_prototype.groups.0.is_empty());
    app.cloud_prototype.production.creating = false;
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn completion_uses_captured_workspace_title_profile_and_revision() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    let destination = app
        .cloud_prototype
        .production
        .pending_creation
        .as_ref()
        .unwrap()
        .workspace
        .clone();
    let _ = app.board.create_workspace("Another workspace");
    app.cloud_prototype.production.title = "Changed title".into();
    app.cloud_prototype.production.selected_profile = "changed".into();
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    let group = &app.cloud_prototype.groups.0[0];
    assert_eq!(group.workspace, destination);
    assert_eq!(group.title, "Captured title");
    let launch = group.remote.as_ref().unwrap();
    assert_eq!(launch.profile_name, "dev");
    assert_eq!(launch.revision, "a".repeat(40));
    assert!(!launch.deployment_started);
    assert!(app.cloud_prototype.production.runtimes.is_empty());
}

#[test]
fn stale_session_or_detached_destination_cannot_create_a_cloud() {
    for stale_session in [true, false] {
        let (temp, mut app) = test_app();
        let sender = pending(&mut app);
        if stale_session {
            app.cloud_prototype
                .production
                .pending_creation
                .as_mut()
                .unwrap()
                .session = Some("old session".into());
        } else {
            let workspace = app.board.ensure_workspace();
            app.detach_workspace(workspace);
        }
        sender.send(Ok(resolved(temp.path()))).unwrap();
        app.poll_cloud_creation(&egui::Context::default());
        assert!(app.cloud_prototype.groups.0.is_empty());
        assert!(app.cloud_prototype.production.runtimes.is_empty());
        assert!(app.cloud_prototype.production.pending_creation.is_none());
    }
}

#[test]
fn resolution_failure_keeps_form_open_without_creating_compute() {
    let (_temp, mut app) = test_app();
    let sender = pending(&mut app);
    sender
        .send(Err(cloud_runtime::Error::Invalid("Missing revision")))
        .unwrap();
    app.poll_cloud_creation(&egui::Context::default());
    assert!(app.cloud_prototype.production.creating);
    assert!(app.cloud_prototype.error.as_ref().unwrap().contains("Missing revision"));
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.production.runtimes.is_empty());
}

#[test]
fn saving_before_initialization_or_after_session_switch_preserves_board_groups() {
    for (initialized, ready, stale) in [(false, false, false), (true, false, false), (true, true, true)] {
        let (temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        app.board.cloud_groups.0.push(CloudGroup::new(
            1,
            "Saved cloud".into(),
            local,
            temp.path().into(),
            [0.0, 0.0],
        ));
        app.cloud_prototype.initialized = initialized;
        app.cloud_prototype.ready = ready;
        if stale {
            app.cloud_prototype.production.session_id = Some("previous session".into());
        }
        app.save_cloud_prototype();
        let state = horizon_core::RuntimeState::from_board(&app.board, app.window_config.clone(), app.canvas_view);
        assert_eq!(state.cloud_groups.0.len(), 1);
        assert_eq!(state.cloud_groups.0[0].title, "Saved cloud");
    }
}

#[test]
fn initialized_current_session_can_persist_removing_the_last_cloud() {
    let (temp, mut app) = test_app();
    let workspace = app.board.ensure_workspace();
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    app.board.cloud_groups.0.push(CloudGroup::new(
        1,
        "Removed".into(),
        local,
        temp.path().into(),
        [0.0, 0.0],
    ));
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.ready = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.save_cloud_prototype();
    assert!(app.board.cloud_groups.0.is_empty());
}

#[test]
fn escape_wins_over_a_result_delivered_in_the_same_frame() {
    use crate::test_egui::DiscardTextures;
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx
            .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
    }
    sender.send(Ok(resolved(temp.path()))).unwrap();
    let mut event = input();
    event.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(event, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures();
    assert!(!app.cloud_prototype.production.creating);
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn cancelled_pending_job_signals_its_worker_and_permit_releases_only_on_exit() {
    let (_temp, mut app) = test_app();
    let _sender = pending(&mut app);
    let cancel = app
        .cloud_prototype
        .production
        .pending_creation
        .as_ref()
        .unwrap()
        .cancel
        .0
        .clone();
    let busy = app.cloud_prototype.production.creation_busy.clone();
    busy.store(true, Ordering::Release);
    let permit = WorkPermit(busy.clone());
    app.cloud_prototype.production.pending_creation = None;
    assert!(cancel.is_cancelled());
    assert!(busy.load(Ordering::Acquire));
    drop(permit);
    assert!(!busy.load(Ordering::Acquire));
}
