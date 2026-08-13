use horizon_core::{CanvasViewState, Config, RuntimeState, StartupDecision, WindowConfig};

use super::super::test_support::{
    editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_config_and_startup,
};

fn runtime_with_staggered_workspaces(prefix: &str) -> RuntimeState {
    RuntimeState {
        workspaces: vec![
            editor_workspace_state(&format!("{prefix}-left"), [100.0, 300.0]),
            editor_workspace_state(&format!("{prefix}-right"), [800.0, 700.0]),
        ],
        ..RuntimeState::default()
    }
}

#[test]
fn persistent_session_switch_waits_for_viewport_before_finalizing_target() {
    let mut config = Config::default();
    config.features.organize_workspaces_on_session_load = true;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_with_staggered_workspaces("old")),
        },
    );
    let old_session = app
        .session_store
        .create_session_from_runtime(runtime_with_staggered_workspaces("old"))
        .expect("old persistent session");
    app.activate_persistent_session(&old_session);
    app.root_viewport_stabilizer = None;
    let target = app
        .session_store
        .create_session_from_runtime(runtime_with_staggered_workspaces("target"))
        .expect("target persistent session");
    let target_before = std::fs::read(&target.runtime_state_path).expect("target runtime before switch");

    app.activate_runtime_session(&ctx, &target);
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([900.0, 700.0], None));

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(app.startup_workspace_organization_pending);
    assert_eq!(
        std::fs::read(&target.runtime_state_path).expect("target runtime while pending"),
        target_before
    );

    app.root_viewport_stabilizer = None;
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let left = app
        .board
        .workspace_id_by_local_id("target-left")
        .and_then(|id| app.board.workspace(id))
        .expect("target left");
    let right = app
        .board
        .workspace_id_by_local_id("target-right")
        .and_then(|id| app.board.workspace(id))
        .expect("target right");
    assert!((left.position[1] - right.position[1]).abs() <= 0.01);
    assert!(app.runtime_dirty_since.is_some());
    assert!(temp.path().join(".horizon").exists());
}

#[test]
fn persisted_target_view_and_window_are_protected_during_session_switch() {
    let config = Config::default();
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_with_staggered_workspaces("old")),
        },
    );
    app.root_viewport_stabilizer = None;
    let mut target_runtime = runtime_with_staggered_workspaces("target");
    target_runtime.canvas_view = Some(CanvasViewState::new([240.0, -90.0], 1.25));
    target_runtime.window = Some(WindowConfig {
        width: 1800.0,
        height: 1100.0,
        x: Some(120.0),
        y: Some(80.0),
    });
    let target = app
        .session_store
        .create_session_from_runtime(target_runtime)
        .expect("target persistent session");
    let target_before = std::fs::read(&target.runtime_state_path).expect("target runtime before switch");

    app.activate_runtime_session(&ctx, &target);
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([900.0, 700.0], None));

    assert!(app.root_viewport_stabilizer.is_some());
    assert!(!app.startup_workspace_organization_pending);
    assert!(app.initial_pan_done);
    assert!((app.window_config.width - 1800.0).abs() <= 0.01);
    assert!((app.window_config.height - 1100.0).abs() <= 0.01);
    assert_eq!(
        std::fs::read(&target.runtime_state_path).expect("target runtime while pending"),
        target_before
    );

    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([1800.0, 1100.0], Some([10.0, 20.0])));
    assert!(app.root_viewport_stabilizer.is_some());
    assert_eq!(app.window_config.x, Some(120.0));
    assert_eq!(app.window_config.y, Some(80.0));
    assert_eq!(
        std::fs::read(&target.runtime_state_path).expect("target runtime while stale position is observed"),
        target_before
    );
}
