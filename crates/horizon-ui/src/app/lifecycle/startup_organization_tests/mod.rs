use egui::{Context, Pos2, RawInput, Rect};
use horizon_core::{
    AgentSessionCatalog, CanvasViewState, Config, DetachedWorkspaceState, PanelKind, RuntimeState, StartupChooser,
    StartupDecision, StartupPromptReason, WindowConfig, WorkspaceState,
};

use crate::app::session::{StartupBootstrap, StartupBootstrapOutcome};
use crate::app::test_support::{
    editor_panel_state, editor_workspace_state, raw_input, run_app_frame, run_app_frame_with_input,
    test_app_with_config_and_startup, test_app_with_startup,
};
use crate::app::{HorizonApp, WS_BG_PAD, WS_TITLE_HEIGHT};
use crate::command_registry::CommandId;

const POSITION_TOLERANCE: f32 = 0.01;

fn enabled_config() -> Config {
    let mut config = Config::default();
    config.features.organize_workspaces_on_session_load = true;
    config
}

fn two_workspace_runtime(canvas_view: Option<CanvasViewState>) -> RuntimeState {
    RuntimeState {
        canvas_view,
        active_workspace_local_id: Some("right".to_string()),
        focused_panel_local_id: Some("right-panel".to_string()),
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [700.0, 500.0]),
        ],
        ..RuntimeState::default()
    }
}

fn enabled_test_app(runtime_state: RuntimeState) -> (tempfile::TempDir, Context, HorizonApp) {
    test_app_with_config_and_startup(
        &enabled_config(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime_state),
        },
    )
}

fn raw_input_without_native_rect(size: [f32; 2]) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]))),
        ..RawInput::default()
    }
}

fn run_frame_at_configured_size(ctx: &Context, app: &mut HorizonApp) {
    app.root_viewport_stabilizer = None;
    let size = [app.window_config.width, app.window_config.height];
    run_app_frame_with_input(ctx, app, raw_input(size, None));
}

fn workspace_position(app: &HorizonApp, local_id: &str) -> [f32; 2] {
    let workspace_id = app
        .board
        .workspace_id_by_local_id(local_id)
        .expect("workspace local id");
    app.board.workspace(workspace_id).expect("workspace").position
}

fn focused_panel_local_id(app: &HorizonApp) -> Option<&str> {
    app.board
        .focused
        .and_then(|panel_id| app.board.panel(panel_id))
        .map(|panel| panel.local_id.as_str())
}

fn active_workspace_local_id(app: &HorizonApp) -> Option<&str> {
    app.board
        .active_workspace
        .and_then(|workspace_id| app.board.workspace(workspace_id))
        .map(|workspace| workspace.local_id.as_str())
}

fn assert_position_near(actual: [f32; 2], expected: [f32; 2]) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() <= POSITION_TOLERANCE),
        "expected {expected:?}, got {actual:?}"
    );
}

fn assert_horizontal_row(app: &HorizonApp, left: &str, right: &str) {
    let left_position = workspace_position(app, left);
    let right_position = workspace_position(app, right);
    assert!((left_position[1] - right_position[1]).abs() <= POSITION_TOLERANCE);
    assert!(right_position[0] > left_position[0]);
}

fn workspace_frame(app: &HorizonApp, local_id: &str) -> (Pos2, egui::Vec2) {
    let workspace_id = app
        .board
        .workspace_id_by_local_id(local_id)
        .expect("workspace local id");
    let (min, max) = app.board.workspace_bounds(workspace_id).expect("workspace bounds");
    (
        Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT),
        egui::vec2(
            max[0] - min[0] + 2.0 * WS_BG_PAD,
            max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
        ),
    )
}

fn assert_workspace_center_is_on_canvas(ctx: &Context, app: &HorizonApp, local_id: &str) {
    let canvas_rect = app.canvas_rect(ctx);
    let (frame_pos, frame_size) = workspace_frame(app, local_id);
    let screen_rect = Rect::from_min_size(
        app.canvas_to_screen(canvas_rect, frame_pos),
        app.canvas_size_to_screen(frame_size),
    );
    assert!(
        canvas_rect.contains(screen_rect.center()),
        "workspace {local_id} center at {:?} is outside canvas {canvas_rect:?}",
        screen_rect.center()
    );
}

mod layout;
mod session_load;
mod viewport;
