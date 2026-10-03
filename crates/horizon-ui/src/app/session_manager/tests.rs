use horizon_core::{CanvasViewState, Config, RuntimeState, StartupDecision, WindowConfig};

use super::super::session_manager::{SessionSwitchShutdownState, session_switch_shutdown_state};
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
fn session_switch_timeout_detaches_terminals_but_never_bypasses_browser_cleanup() {
    assert_eq!(
        session_switch_shutdown_state(false, true, true),
        SessionSwitchShutdownState::Complete
    );
    assert_eq!(
        session_switch_shutdown_state(false, false, true),
        SessionSwitchShutdownState::AbortForBrowser
    );
    assert_eq!(
        session_switch_shutdown_state(false, false, false),
        SessionSwitchShutdownState::Waiting
    );
    assert_eq!(
        session_switch_shutdown_state(true, true, false),
        SessionSwitchShutdownState::Complete
    );
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
    if app.runtime_dirty_since.is_some() {
        app.runtime_dirty_since = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .expect("test clock supports a one-second lookback"),
        );
        app.flush_runtime_if_dirty();
    }
    let saved_target = RuntimeState::load(&target.runtime_state_path)
        .expect("load target runtime after switch")
        .expect("target runtime exists after switch");
    let saved_left = saved_target
        .workspaces
        .iter()
        .find(|workspace| workspace.local_id == "target-left")
        .expect("saved target left");
    let saved_right = saved_target
        .workspaces
        .iter()
        .find(|workspace| workspace.local_id == "target-right")
        .expect("saved target right");
    let saved_left_y = saved_left.position.expect("saved target left position")[1];
    let saved_right_y = saved_right.position.expect("saved target right position")[1];
    assert!((saved_left_y - saved_right_y).abs() <= 0.01);
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

#[cfg(target_os = "linux")]
#[test]
fn session_switch_cancels_native_input_before_panel_ids_are_reused() {
    #[derive(Debug)]
    struct File(std::path::PathBuf);
    impl egui::DroppedFile for File {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Ok(b"synthetic stale drop".to_vec())
        }
    }
    let config = Config::default();
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_with_staggered_workspaces("old")),
        },
    );
    app.root_viewport_stabilizer = None;
    let target = app
        .session_store
        .create_session_from_runtime(runtime_with_staggered_workspaces("target"))
        .expect("target session");
    let input = app.observed_keyboard_inputs.clone();
    input.native_context(&ctx);
    let files: Vec<egui::DroppedFileHandle> = vec![std::sync::Arc::new(File(std::path::PathBuf::from("/tmp/old.txt")))];
    ctx.input_mut(|input| input.raw.dropped_files = files.clone());

    input.native_window_seen(10);
    input.native_focus(10, true);
    input.native_recipient_publisher()(egui::ViewportId::ROOT, 1.0, Some(horizon_core::PanelId(1)));
    let queued = input.native_paste_request(10).expect("queued request");
    let pending = input.native_paste_request(10).expect("pending request");
    input.native_paste(queued, vec![std::path::PathBuf::from("/tmp/old.txt")]);
    input.native_drop_position(10, [10.0, 20.0], vec![std::path::PathBuf::from("/tmp/old.txt")]);
    app.begin_session_switch(&target);
    assert!(ctx.input(|input| input.raw.dropped_files.is_empty()));
    assert!(
        input
            .take_native_drop_position(egui::ViewportId::ROOT, &files)
            .is_none()
    );
    input.native_paste(pending, vec![std::path::PathBuf::from("/tmp/late.png")]);
    assert!(input.take_native_pastes().is_empty());
    assert!(input.native_paste_request(10).is_none());
    let _ = app.poll_session_switch(&ctx);
    input.set_wayland_backend(true);
    ctx.input_mut(|input| input.raw.dropped_files = files);
    let panels = app.board.panels.len();
    app.handle_root_file_drop(&ctx);
    assert_eq!(
        app.board.panels.len(),
        panels,
        "stale raw drop opened a file on the replacement board"
    );
    input.native_paste(pending, vec![std::path::PathBuf::from("/tmp/later.png")]);
    assert!(input.take_native_pastes().is_empty());
}
