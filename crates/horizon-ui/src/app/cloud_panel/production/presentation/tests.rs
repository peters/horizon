use super::*;
use crate::app::cloud_panel::production::Runtime;
#[cfg(unix)]
use crate::app::test_support::test_app;
#[cfg(unix)]
use horizon_core::{
    Board, BrowserProfileState, PanelState, RuntimeState, WorkspaceState,
    browser::{BackendKind, CloudViewState},
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
};

#[test]
#[cfg(unix)]
fn cloud_browser_restore_waits_for_discovery_and_preserves_engines_through_autosave() {
    restore_scenario(false);
}

#[test]
#[cfg(unix)]
fn lost_cloud_browser_leaves_reconnecting_placeholder_without_recreating_identity() {
    restore_scenario(true);
}

#[cfg(unix)]
fn restore_scenario(lost: bool) {
    let (_temp, mut app) = restore_fixture();
    app.sync_cloud_presentations();
    for local in ["firefox", "chromium"] {
        assert!(
            app.board
                .panel(app.board.panel_id_by_local_id(local).unwrap())
                .unwrap()
                .browser()
                .is_none(),
            "Ready alone must not start a browser"
        );
    }
    let roundtrip = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    let roundtrip: RuntimeState = serde_yaml::from_str(&roundtrip.to_yaml().unwrap()).unwrap();
    assert_eq!(
        roundtrip.workspaces[0].panels[0]
            .browser_profile
            .as_ref()
            .unwrap()
            .backend,
        Some(BackendKind::FirefoxBidi)
    );
    assert!(
        roundtrip.workspaces[0].panels[0]
            .browser_profile
            .as_ref()
            .unwrap()
            .hidden
    );
    assert_eq!(
        roundtrip.workspaces[0].panels[0].browser_url.as_deref(),
        Some("http://example.invalid/saved")
    );
    app.board = Board::from_runtime_state(&roundtrip).unwrap();
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().browsers = Some(
        [
            ("firefox", BackendKind::FirefoxBidi),
            ("chromium", BackendKind::ChromiumCdp),
        ]
        .into_iter()
        .map(|(id, backend)| CloudViewState {
            id: id.into(),
            backend,
            visible: true,
            ready: !lost,
            lost,
            ..Default::default()
        })
        .collect(),
    );
    app.sync_cloud_presentations();
    for (local, backend) in [
        ("firefox", BackendKind::FirefoxBidi),
        ("chromium", BackendKind::ChromiumCdp),
    ] {
        let panel = app.board.panel(app.board.panel_id_by_local_id(local).unwrap()).unwrap();
        assert_eq!(panel.browser().unwrap().backend(), backend);
        assert_eq!(panel.local_id, local);
    }
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_browser_attachments
            .is_empty()
    );
    // A presentation left behind by an older client is repaired from worker discovery.
    let id = app.board.panel_id_by_local_id("firefox").unwrap();
    let workspace = app.board.panel(id).unwrap().workspace_id;
    let mut options = PanelOptions {
        kind: PanelKind::Browser,
        local_id: Some("firefox".into()),
        ..Default::default()
    };
    app.prepare_cloud_remote_panel(0, &mut options).unwrap();
    options.browser_config.as_mut().unwrap().backend = BackendKind::ChromiumCdp;
    let previous = app.board.panel_mut(id).unwrap();
    previous.request_shutdown();
    *previous = Panel::spawn(id, workspace, options).unwrap();
    app.sync_cloud_presentations();
    assert_eq!(
        app.board.panel(id).unwrap().browser().unwrap().backend(),
        BackendKind::FirefoxBidi
    );
    app.sync_cloud_presentations();
    assert_eq!(app.cloud_prototype.groups.0[0].panels, ["firefox", "chromium"]);
}

#[cfg(unix)]
fn restore_fixture() -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = test_app();
    let profile = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker\n    cpu: 4\n    memory_gb: 8\n    capabilities:\n      browsers: [chromium, firefox]\n").unwrap().profiles["dev"].clone();
    let mut group = CloudGroup::new(1, "Cloud".into(), "workspace".into(), temp.path().into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: profile.clone(),
    });
    group.panels = vec!["firefox".into(), "chromium".into()];
    let mut saved = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".into(),
            name: "Fixture".into(),
            panels: [
                ("firefox", BackendKind::FirefoxBidi),
                ("chromium", BackendKind::ChromiumCdp),
            ]
            .into_iter()
            .map(|(id, backend)| PanelState {
                local_id: id.into(),
                kind: PanelKind::Browser,
                browser_url: Some("http://example.invalid/saved".into()),
                browser_profile: Some(BrowserProfileState {
                    backend: Some(backend),
                    hidden: id == "firefox",
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    saved.cloud_groups.0.push(group.clone());
    app.board = Board::from_runtime_state(&saved).unwrap();
    app.cloud_prototype.groups = saved.cloud_groups.clone();
    app.cloud_prototype.root = Some(temp.path().into());
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"fixture","repository":temp.path(),"revision":"a".repeat(40),"profile":profile,
        "stage":"Ready","operation":{"state":"bound","worker_id":"fixture"},"spec":null,
        "worker":{"id":"fixture","name":"fixture","imageName":"example/worker","desiredStatus":"RUNNING","publicIp":"127.0.0.1","portMappings":{"22":9}},
        "sessions":[],"source_ready":true
    })).unwrap();
    let store = cloud_runtime::state::Store::lock(&temp.path().join("fixture")).unwrap();
    store.save(&state).unwrap();
    drop(store);
    std::fs::write(temp.path().join("settings.json"), serde_json::to_vec(&serde_json::json!({
        "runpod_key_file":temp.path().join("unused-key"),"ssh_identity_file":temp.path().join("absent-identity"),"docker_config":temp.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
    })).unwrap()).unwrap();
    app.cloud_prototype.production.runtimes.insert(
        1,
        Runtime {
            state: Some(state),
            needs_attach: true,
            ..Default::default()
        },
    );
    (temp, app)
}

#[test]
#[cfg(unix)]
fn empty_worker_discovery_reports_missing_process_without_opening_a_replacement() {
    let (_temp, mut app) = restore_fixture();
    let mut saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    saved.workspaces[0].panels[0]
        .browser_profile
        .as_mut()
        .unwrap()
        .remote_target = Some("phone".into());
    app.board = Board::from_runtime_state(&saved).unwrap();
    app.sync_cloud_presentations();
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().browsers = Some(Vec::new());
    app.sync_cloud_presentations();
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id("firefox").unwrap())
        .unwrap();
    assert!(panel.browser().is_none());
    assert_eq!(panel.browser_backend(), Some(BackendKind::FirefoxBidi));
    assert_eq!(panel.browser_remote_target(), Some("phone"));
    let persisted = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    let persisted: RuntimeState = serde_yaml::from_str(&persisted.to_yaml().unwrap()).unwrap();
    let profile = persisted.workspaces[0].panels[0].browser_profile.as_ref().unwrap();
    assert_eq!(profile.remote_target.as_deref(), Some("phone"));
    assert_eq!(profile.backend, Some(BackendKind::FirefoxBidi));
    let text = panel.terminal().unwrap().full_text_lines(100).0.join("\n");
    assert!(text.contains("Remote browser process is no longer available"));
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_browser_attachments
            .is_empty()
    );
}

#[test]
#[cfg(unix)]
fn fullscreen_panel_continues_processing_cloud_ownership_and_lifecycle_events() {
    use crate::test_egui::DiscardTextures;
    let (_temp, mut app) = restore_fixture();
    let ctx = egui::Context::default();
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.ready = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|s| s.session_id.clone());
    app.pending_startup_runtime_state = None;
    app.startup_receiver = None;
    let panel = app.board.panel_id_by_local_id("chromium").unwrap();
    app.fullscreen_panel = Some(panel);
    let (tx, rx) = std::sync::mpsc::channel();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    runtime.state = None;
    runtime.needs_attach = false;
    runtime.receiver = Some(rx);
    let actor = "horizon:cloud-native-agent";
    tx.send(Event::DesktopControl {
        active: Some(actor.into()),
        last: Some(actor.into()),
    })
    .unwrap();
    tx.send(Event::stage(cloud_runtime::Stage::Readiness)).unwrap();
    let draw = |app: &mut HorizonApp| {
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    ..Default::default()
                },
                |ui| app.render_active_view(ui, false),
            )
            .discard_textures();
    };
    draw(&mut app);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.desktop_controller.as_deref(), Some(actor));
    assert_eq!(runtime.stage, Some(cloud_runtime::Stage::Readiness));
    assert_eq!(app.fullscreen_panel, Some(panel));
    tx.send(Event::DesktopControl {
        active: None,
        last: Some(actor.into()),
    })
    .unwrap();
    tx.send(Event::failed("Synthetic readiness failure".into())).unwrap();
    draw(&mut app);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.desktop_controller.is_none());
    assert_eq!(runtime.desktop_last_input.as_deref(), Some(actor));
    assert_eq!(runtime.error.as_deref(), Some("Synthetic readiness failure"));
    assert_eq!(app.fullscreen_panel, Some(panel));
}

#[test]
#[cfg(unix)]
fn lost_browser_placeholder_can_be_dismissed_without_remote_release() {
    let (_temp, mut app) = restore_fixture();
    app.sync_cloud_presentations();
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().browsers = Some(Vec::new());
    app.sync_cloud_presentations();
    let id = app.board.panel_id_by_local_id("firefox").unwrap();
    assert!(app.board.panel(id).unwrap().browser().is_none());
    assert!(!app.close_cloud_browser(id));
    assert!(app.cloud_prototype.error.is_none());
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .state
            .as_ref()
            .unwrap()
            .worker
            .is_some()
    );
}

#[cfg(unix)]
fn add_restored_member(app: &mut HorizonApp, local: &str, kind: PanelKind) {
    let mut saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    saved.workspaces[0].panels.push(PanelState {
        local_id: local.into(),
        kind,
        ..Default::default()
    });
    app.cloud_prototype.groups.0[0].panels.push(local.into());
    saved.cloud_groups = app.cloud_prototype.groups.clone();
    app.board = Board::from_runtime_state(&saved).unwrap();
}

#[test]
#[cfg(unix)]
fn failed_member_attachment_retries_without_replacing_successful_terminal() {
    let (temp, mut app) = restore_fixture();
    add_restored_member(&mut app, "first-shell", PanelKind::Shell);
    add_restored_member(&mut app, "second-shell", PanelKind::Shell);
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_member_attachments
            .is_empty()
    );
    let first = app.board.panel_id_by_local_id("first-shell").unwrap();
    app.board
        .panel_mut(first)
        .unwrap()
        .terminal_mut()
        .unwrap()
        .resize_immediately(17, 37, 8, 16);
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .pending_member_attachments
        .insert("second-shell".into());
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .next_attachment_attempt = None;
    let lock = cloud_runtime::state::Store::lock(&temp.path().join("fixture")).unwrap();
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_member_attachments
            .contains("second-shell")
    );
    assert_eq!(app.board.panel(first).unwrap().terminal().unwrap().cols(), 37);
    drop(lock);
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .next_attachment_attempt = None;
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_member_attachments
            .is_empty()
    );
    assert_eq!(app.board.panel(first).unwrap().terminal().unwrap().cols(), 37);
}

#[test]
#[cfg(unix)]
fn missing_durable_session_retries_after_store_lock_is_released() {
    let (temp, mut app) = restore_fixture();
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .sessions
        .push(cloud_runtime::state::Session {
            panel_id: "missing-shell".into(),
            agent: "shell".into(),
            tmux: "missing-shell".into(),
            branch: "agent/missing-shell".into(),
            worktree: "/workspace/agents/missing-shell".into(),
        });
    let lock = cloud_runtime::state::Store::lock(&temp.path().join("fixture")).unwrap();
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_session_attachments
            .contains("missing-shell")
    );
    assert!(app.board.panel_id_by_local_id("missing-shell").is_none());
    drop(lock);
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .next_attachment_attempt = None;
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_session_attachments
            .is_empty()
    );
    assert!(app.board.panel_id_by_local_id("missing-shell").is_some());
    assert!(
        app.cloud_prototype.groups.0[0]
            .panels
            .contains(&"missing-shell".to_owned())
    );
}

#[test]
#[cfg(unix)]
fn unavailable_desktop_attachment_stays_pending() {
    let (_temp, mut app) = restore_fixture();
    add_restored_member(&mut app, "desktop", PanelKind::Device);
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .needs_desktop = true;
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_member_attachments
            .contains("desktop")
    );
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .next_attachment_attempt = None;
    app.sync_cloud_presentations();
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_member_attachments
            .contains("desktop")
    );
}

#[test]
#[cfg(unix)]
fn pending_session_retry_does_not_reopen_a_closed_healthy_view() {
    let (temp, mut app) = restore_fixture();
    add_restored_member(&mut app, "healthy-shell", PanelKind::Shell);
    let sessions = ["healthy-shell", "missing-shell"].map(|id| cloud_runtime::state::Session {
        panel_id: id.into(),
        agent: "shell".into(),
        tmux: id.into(),
        branch: format!("agent/{id}"),
        worktree: format!("/workspace/agents/{id}"),
    });
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .sessions = sessions.into();
    let lock = cloud_runtime::state::Store::lock(&temp.path().join("fixture")).unwrap();
    app.sync_cloud_presentations();
    assert_eq!(
        app.cloud_prototype.production.runtimes[&1].pending_session_attachments,
        ["missing-shell".to_owned()].into()
    );
    let healthy = app.board.panel_id_by_local_id("healthy-shell").unwrap();
    app.board.close_panel(healthy);
    drop(lock);
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .next_attachment_attempt = None;
    app.sync_cloud_presentations();
    assert!(app.board.panel_id_by_local_id("healthy-shell").is_none());
    assert!(app.board.panel_id_by_local_id("missing-shell").is_some());
    assert!(
        app.cloud_prototype.production.runtimes[&1]
            .pending_session_attachments
            .is_empty()
    );
}

#[test]
#[cfg(unix)]
fn browser_attachment_keeps_repainting_after_discovery_watch_stops() {
    let (temp, mut app) = restore_fixture();
    app.sync_cloud_presentations();
    let runtime = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    runtime.receiver = None;
    runtime.needs_attach = false;
    runtime.pending_member_attachments.clear();
    runtime.pending_session_attachments.clear();
    runtime.browsers = Some(
        [
            ("firefox", BackendKind::FirefoxBidi),
            ("chromium", BackendKind::ChromiumCdp),
        ]
        .into_iter()
        .map(|(id, backend)| CloudViewState {
            id: id.into(),
            backend,
            ready: true,
            ..Default::default()
        })
        .collect(),
    );
    let settings = temp.path().join("settings.json");
    let bytes = std::fs::read(&settings).unwrap();
    std::fs::remove_file(&settings).unwrap();
    app.sync_cloud_presentations();
    assert!(app.cloud_prototype.production.runtimes[&1].needs_repaint());
    assert!(
        !app.cloud_prototype.production.runtimes[&1]
            .pending_browser_attachments
            .is_empty()
    );
    std::fs::write(settings, bytes).unwrap();
    app.sync_cloud_presentations();
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.pending_browser_attachments.is_empty());
    assert!(!runtime.needs_repaint());
    for local in ["firefox", "chromium"] {
        assert!(
            app.board
                .panel(app.board.panel_id_by_local_id(local).unwrap())
                .unwrap()
                .browser()
                .is_some()
        );
    }
}

#[test]
#[cfg(unix)]
fn dismissing_pending_browser_placeholders_stops_repaint_retries() {
    let (_temp, mut app) = restore_fixture();
    app.sync_cloud_presentations();
    assert!(app.cloud_prototype.production.runtimes[&1].needs_repaint());
    for local in ["firefox", "chromium"] {
        app.board.close_panel(app.board.panel_id_by_local_id(local).unwrap());
    }
    app.sync_cloud_presentations();
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.receiver.is_none());
    assert!(runtime.pending_browser_attachments.is_empty());
    assert!(!runtime.needs_repaint());
}

#[test]
fn ready_discovery_receiver_does_not_schedule_idle_repaint() {
    let (_sender, receiver) = std::sync::mpsc::channel();
    let mut runtime = Runtime {
        receiver: Some(receiver),
        stage: Some(cloud_runtime::Stage::Ready),
        ..Default::default()
    };
    assert!(!runtime.needs_repaint());
    runtime.stage = Some(cloud_runtime::Stage::Readiness);
    assert!(runtime.needs_repaint());
    runtime.stage = Some(cloud_runtime::Stage::Ready);
    runtime.pending_browser_attachments.insert("retry".into());
    assert!(runtime.needs_repaint());
}

#[test]
#[cfg(unix)]
fn browser_cleanup_preserves_cloud_restore_and_failed_presentations() {
    use horizon_core::browser::{BrowserPanelState, BrowserStatus};

    let (_temp, mut app) = restore_fixture();
    let id = app.board.panel_id_by_local_id("firefox").unwrap();
    let workspace = app.board.panel(id).unwrap().workspace_id;
    // A legacy restored browser may have an error and no active presentation yet.
    let mut restored = BrowserPanelState::inert();
    restored.status = BrowserStatus::Error {
        message: "Waiting for cloud attachment".into(),
    };
    let panel = app.board.panel_mut(id).unwrap();
    *panel = Panel::from_content(
        id,
        workspace,
        PanelKind::Browser,
        horizon_core::PanelContent::Browser(Box::new(restored)),
    );
    panel.local_id = "firefox".into();
    app.fullscreen_panel = Some(id);
    let groups = std::mem::take(&mut app.cloud_prototype.groups);
    app.poll_browser_create_requests();
    assert!(
        app.board.panel(id).is_some(),
        "saved membership must protect early restore"
    );
    assert_eq!(app.fullscreen_panel, Some(id));
    app.cloud_prototype.groups = groups;
    assert!(!app.close_ended_browser_panels());
    assert!(
        app.cloud_prototype.error.is_none(),
        "cleanup must not attempt remote Close"
    );

    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().browsers = Some(vec![CloudViewState {
        id: "firefox".into(),
        backend: BackendKind::FirefoxBidi,
        ready: true,
        visible: true,
        ..Default::default()
    }]);
    app.sync_cloud_presentations();
    let browser = app.board.panel_mut(id).unwrap().browser_mut().unwrap();
    assert_eq!(browser.backend(), BackendKind::FirefoxBidi);
    browser.status = BrowserStatus::Error {
        message: "Presentation transport interrupted".into(),
    };
    assert!(!browser.has_ended());
    assert!(!app.close_ended_browser_panels());
    assert!(
        app.cloud_prototype.error.is_none(),
        "no implicit remote Close on transport loss"
    );
    assert_eq!(app.board.panel_id_by_local_id("firefox"), Some(id));
    assert_eq!(app.fullscreen_panel, Some(id));
}

#[test]
#[cfg(unix)]
fn cloud_device_identity_survives_placeholder_autosave_and_reattachment() {
    use horizon_core::browser::manifest::device::DeviceIdentity;
    let (_temp, mut app) = restore_fixture();
    let identity = DeviceIdentity {
        machine_name: Some("Synthetic worker".into()),
        hostname: Some("worker.invalid".into()),
        ..Default::default()
    };
    let mut saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    saved.workspaces[0].panels.push(PanelState {
        local_id: "desktop".into(),
        kind: PanelKind::Device,
        command: Some("127.0.0.1:5900".into()),
        device_identity: Some(identity.clone()),
        ..Default::default()
    });
    saved.cloud_groups.0[0].panels.push("desktop".into());
    app.cloud_prototype.groups = saved.cloud_groups.clone();
    app.board = Board::from_runtime_state(&saved).unwrap();
    let id = app.board.panel_id_by_local_id("desktop").unwrap();
    assert!(app.board.panel(id).unwrap().device().is_none());
    assert_eq!(app.board.panel(id).unwrap().device_identity(), Some(&identity));
    let snapshot = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    let snapshot: RuntimeState = serde_yaml::from_str(&snapshot.to_yaml().unwrap()).unwrap();
    assert_eq!(snapshot.workspaces[0].panels[2].device_identity, Some(identity.clone()));
    app.board = Board::from_runtime_state(&snapshot).unwrap();
    let id = app.board.panel_id_by_local_id("desktop").unwrap();
    // Metadata replacement is independent of SSH tunnel establishment.
    app.cloud_prototype.groups.0[0].remote = None;
    for _ in 0..2 {
        assert!(app.restore_cloud_member(0, id, false));
        let panel = app.board.panel(id).unwrap();
        assert_eq!(panel.local_id, "desktop");
        assert_eq!(panel.device().unwrap().identity, Some(identity.clone()));
    }
}
