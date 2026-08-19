use super::*;
use crate::test_egui::DiscardTextures;

#[test]
fn viewport_stabilization_keeps_rendering_without_native_geometry() {
    let mut runtime_state = two_workspace_runtime(None);
    runtime_state.window = Some(WindowConfig {
        width: 2560.0,
        height: 1440.0,
        x: None,
        y: None,
    });
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    let moving_panel = app.board.panel_id_by_local_id("right-panel").expect("right panel");
    app.workspace_creates.push(moving_panel);

    let output = run_app_frame_with_input(&ctx, &mut app, raw_input_without_native_rect([1400.0, 900.0]));

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(app.startup_workspace_organization_pending);
    assert!(!app.initial_pan_done);
    assert_eq!(
        app.board.workspaces.len(),
        3,
        "normal frame mutations must keep running"
    );
    assert!(
        !output.shapes.is_empty(),
        "pending stabilization must still render the active view"
    );
}

#[test]
fn count_change_during_viewport_stabilization_is_retried_afterward() {
    let (temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "persistent-count-change".to_string(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    app.theme_applied = true;
    let moving_panel = app.board.panel_id_by_local_id("right-panel").expect("right panel");
    app.workspace_creates.push(moving_panel);

    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input_without_native_rect([1400.0, 900.0]));

    let runtime_path = temp
        .path()
        .join(".horizon/sessions/persistent-count-change/runtime.yaml");
    assert!(app.root_viewport_stabilizer.is_some());
    assert!(
        app.runtime_dirty_since.is_some(),
        "blocked count save must be queued for retry"
    );
    assert!(!runtime_path.exists());

    app.root_viewport_stabilizer = None;
    app.runtime_dirty_since = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("test clock supports a one-second lookback"),
    );
    app.flush_runtime_if_dirty();

    let saved = RuntimeState::load(&runtime_path)
        .expect("load retried runtime")
        .expect("retried runtime exists");
    assert_eq!(saved.workspaces.len(), 3);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn viewport_stabilization_blocks_global_session_ui_shortcuts() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    app.theme_applied = true;
    let mut input = raw_input_without_native_rect([1400.0, 900.0]);
    let modifiers = egui::Modifiers {
        ctrl: !cfg!(target_os = "macos"),
        shift: true,
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
        ..egui::Modifiers::NONE
    };
    for key in [egui::Key::J, egui::Key::K] {
        input.events.push(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        });
    }

    let output = run_app_frame_with_input(&ctx, &mut app, input);

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(app.session_manager.is_none());
    assert!(app.command_palette.is_none());
    assert!(!output.shapes.is_empty(), "the blocked frame must still render");
}

#[test]
fn viewport_stabilization_is_non_modal_when_organization_is_disabled() {
    let (_temp, _ctx, mut app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(two_workspace_runtime(None)),
        },
    );

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(!app.startup_workspace_organization_pending);
    assert!(!app.root_viewport_stabilization_blocks_interaction());
    app.open_command_palette();
    assert!(app.command_palette.is_some());
    app.toggle_session_manager();
    assert!(app.session_manager.is_some());
}

#[test]
fn disabled_organization_seeds_initial_view_before_accepting_root_input() {
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(two_workspace_runtime(None)),
        },
    );
    app.theme_applied = true;
    let mut input = raw_input_without_native_rect([1400.0, 900.0]);
    input
        .events
        .push(egui::Event::PointerMoved(egui::Pos2::new(400.0, 300.0)));
    input.events.push(egui::Event::Zoom(1.25));

    let _ = run_app_frame_with_input(&ctx, &mut app, input);

    assert!(app.initial_pan_done);
    assert!(app.root_viewport_stabilizer.is_some());
    assert!((app.canvas_view.zoom - 1.25).abs() <= f32::EPSILON);
    let accepted_view = app.canvas_view;

    app.root_viewport_stabilizer = None;
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input_without_native_rect([1400.0, 900.0]));
    assert_eq!(app.canvas_view, accepted_view);
}

#[test]
fn viewport_stabilization_suppresses_raw_root_interaction() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    let mut input = raw_input_without_native_rect([1400.0, 900.0]);
    input.events.push(egui::Event::Text("blocked".to_string()));
    input.events.push(egui::Event::PointerButton {
        pos: egui::Pos2::new(400.0, 300.0),
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    input.events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(30.0, 40.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    input.hovered_files.push(egui::HoveredFile::default());
    input
        .dropped_files
        .push(crate::app::test_support::dropped_file("/tmp/blocked.txt"));

    let _ = ctx
        .run_ui(input, |ui| {
            app.suppress_root_viewport_interaction(ui);
            ui.input(|input| {
                assert!(input.raw.events.is_empty());
                assert!(input.raw.hovered_files.is_empty());
                assert!(input.raw.dropped_files.is_empty());
                assert!(input.events.is_empty());
                assert!(input.keys_down.is_empty());
                assert!(!input.pointer.primary_down());
                assert_eq!(input.smooth_scroll_delta, egui::Vec2::ZERO);
            });
        })
        .discard_textures();
}

#[test]
fn viewport_stabilization_drops_root_zoom_before_canvas_handling() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    app.theme_applied = true;
    let canvas_view_before = app.canvas_view;
    let mut input = raw_input_without_native_rect([1400.0, 900.0]);
    input
        .events
        .push(egui::Event::PointerMoved(egui::Pos2::new(400.0, 300.0)));
    input.events.push(egui::Event::Zoom(1.25));

    let _ = run_app_frame_with_input(&ctx, &mut app, input);

    assert_eq!(app.canvas_view, canvas_view_before);
    assert!(app.root_viewport_stabilizer.is_some());
}

#[test]
fn viewport_stabilization_keeps_detached_viewports_alive() {
    let mut runtime_state = two_workspace_runtime(None);
    runtime_state.detached_workspaces = vec![DetachedWorkspaceState {
        workspace_local_id: "right".to_string(),
        window: WindowConfig::default(),
    }];
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;

    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input_without_native_rect([1400.0, 900.0]));

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(
        !app.detached_workspaces
            .get("right")
            .expect("detached workspace")
            .initial_fit_pending,
        "detached rendering must keep running on every pending root pass"
    );
}

#[test]
fn blocked_runtime_save_retains_the_dirty_marker() {
    let (_temp, _ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "persistent-save-test".to_string(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    let initial_dirty_since = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .expect("test clock supports a one-second lookback");
    app.runtime_dirty_since = Some(initial_dirty_since);
    app.arm_root_viewport_stabilizer(false, [app.window_config.width, app.window_config.height]);

    app.flush_runtime_if_dirty();
    assert!(
        app.runtime_dirty_since
            .is_some_and(|retry_since| retry_since > initial_dirty_since)
    );

    app.root_viewport_stabilizer = None;
    app.runtime_dirty_since = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("test clock supports a one-second lookback"),
    );
    app.flush_runtime_if_dirty();
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn non_modal_stabilization_can_save_user_changes() {
    let (temp, _ctx, mut app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(two_workspace_runtime(None)),
        },
    );
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "persistent-feature-off-save".to_string(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    app.canvas_view = CanvasViewState::new([42.0, -18.0], 1.25);

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(!app.root_viewport_stabilization_blocks_interaction());
    assert!(app.auto_save_runtime_state());

    let saved = RuntimeState::load(
        &temp
            .path()
            .join(".horizon/sessions/persistent-feature-off-save/runtime.yaml"),
    )
    .expect("load saved feature-off runtime")
    .expect("feature-off runtime exists");
    assert_eq!(saved.canvas_view, Some(app.canvas_view));
}

#[test]
fn shutdown_during_viewport_stabilization_leaves_the_prior_snapshot_untouched() {
    let (temp, _ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "persistent-shutdown-test".to_string(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    app.runtime_dirty_since = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("test clock supports a one-second lookback"),
    );
    app.arm_root_viewport_stabilizer(false, [app.window_config.width, app.window_config.height]);
    let runtime_path = temp
        .path()
        .join(".horizon/sessions/persistent-shutdown-test/runtime.yaml");

    app.run_exit_cleanup();

    assert!(app.runtime_dirty_since.is_some());
    assert!(app.exit_cleanup_complete);
    assert!(
        !runtime_path.exists(),
        "shutdown must not create a partial-session snapshot"
    );
}
