use super::*;

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
    app.runtime_dirty_since = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .expect("test clock supports a one-second lookback"),
    );
    app.arm_root_viewport_stabilizer(false);

    app.flush_runtime_if_dirty();
    assert!(app.runtime_dirty_since.is_some());

    app.root_viewport_stabilizer = None;
    app.flush_runtime_if_dirty();
    assert!(app.runtime_dirty_since.is_none());
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
    app.arm_root_viewport_stabilizer(false);
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
