use egui::{Context, Pos2, RawInput, Rect, ViewportId};
use horizon_core::{
    AgentSessionCatalog, CanvasViewState, Config, DetachedWorkspaceState, PanelKind, PanelState, RuntimeState,
    StartupChooser, StartupDecision, StartupPromptReason, WindowConfig, WorkspaceState,
};

use super::HorizonApp;
use crate::app::session::{StartupBootstrap, StartupBootstrapOutcome};
use crate::app::test_support::{
    run_app_frame, run_app_frame_with_input, test_app_with_config_and_startup, test_app_with_startup,
};
use crate::app::{WS_BG_PAD, WS_TITLE_HEIGHT};
use crate::command_registry::CommandId;

const POSITION_TOLERANCE: f32 = 0.01;

fn enabled_config() -> Config {
    let mut config = Config::default();
    config.features.organize_workspaces_on_startup = true;
    config
}

fn editor_panel_state(local_id: &str, position: [f32; 2]) -> PanelState {
    PanelState {
        local_id: local_id.to_string(),
        name: format!("{local_id} notes"),
        kind: PanelKind::Editor,
        position: Some(position),
        size: Some([320.0, 220.0]),
        ..PanelState::default()
    }
}

fn editor_workspace_state(local_id: &str, position: [f32; 2]) -> WorkspaceState {
    WorkspaceState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        position: Some(position),
        panels: vec![editor_panel_state(
            &format!("{local_id}-panel"),
            [position[0] + 20.0, position[1] + 60.0],
        )],
        ..WorkspaceState::default()
    }
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

fn raw_input(size: [f32; 2], position: Option<[f32; 2]>) -> RawInput {
    let inner_rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]));
    let mut input = RawInput {
        screen_rect: Some(inner_rect),
        ..RawInput::default()
    };
    let viewport = input.viewports.entry(ViewportId::ROOT).or_default();
    viewport.inner_rect = Some(inner_rect);
    viewport.outer_rect =
        position.map(|position| Rect::from_min_size(Pos2::new(position[0], position[1]), egui::vec2(size[0], size[1])));
    input
}

fn run_frame_at_configured_size(ctx: &Context, app: &mut HorizonApp) {
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

fn assert_workspace_intersects_canvas(ctx: &Context, app: &HorizonApp, local_id: &str) {
    let workspace_id = app
        .board
        .workspace_id_by_local_id(local_id)
        .expect("workspace local id");
    let (min, max) = app.board.workspace_bounds(workspace_id).expect("workspace bounds");
    let canvas_rect = app.canvas_rect(ctx);
    let frame_pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
    let frame_size = egui::vec2(
        max[0] - min[0] + 2.0 * WS_BG_PAD,
        max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
    );
    let screen_rect = Rect::from_min_size(
        app.canvas_to_screen(canvas_rect, frame_pos),
        app.canvas_size_to_screen(frame_size),
    );
    assert!(
        screen_rect.intersects(canvas_rect),
        "workspace {local_id} at {screen_rect:?} is outside canvas {canvas_rect:?}"
    );
}

#[test]
fn startup_organization_is_disabled_by_default() {
    let runtime_state = two_workspace_runtime(Some(CanvasViewState::new([24.0, -12.0], 1.0)));
    let expected_left = runtime_state.workspaces[0].position.expect("left position");
    let expected_right = runtime_state.workspaces[1].position.expect("right position");
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state),
    });
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_position_near(workspace_position(&app, "left"), expected_left);
    assert_position_near(workspace_position(&app, "right"), expected_right);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn startup_frame_organizes_attached_workspaces_only_once() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);
    assert_horizontal_row(&app, "left", "right");

    let right_id = app.board.workspace_id_by_local_id("right").expect("right workspace");
    assert!(app.board.translate_workspace(right_id, [0.0, 137.0]));
    let moved_position = workspace_position(&app, "right");
    app.runtime_dirty_since = None;
    run_frame_at_configured_size(&ctx, &mut app);

    assert_position_near(workspace_position(&app, "right"), moved_position);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn startup_frame_preserves_persisted_canvas_view() {
    let saved_view = CanvasViewState::new([220.0, -80.0], 1.25);
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(saved_view)));
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(app.canvas_view, saved_view);
    assert!(app.pan_target.is_none());
    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn startup_frame_keeps_a_visible_focused_workspace_on_screen() {
    let saved_view = CanvasViewState::new([-4_700.0, -250.0], 1.0);
    let runtime_state = RuntimeState {
        canvas_view: Some(saved_view),
        active_workspace_local_id: Some("right".to_string()),
        focused_panel_local_id: Some("right-panel".to_string()),
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [5_000.0, 500.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
    assert_workspace_intersects_canvas(&ctx, &app, "right");
    assert!(
        app.canvas_view
            .pan_offset
            .iter()
            .zip(saved_view.pan_offset)
            .any(|(current, saved)| (current - saved).abs() > POSITION_TOLERANCE)
    );
    assert!(app.pan_target.is_none());
    assert_eq!(focused_panel_local_id(&app), Some("right-panel"));
    assert_eq!(active_workspace_local_id(&app), Some("right"));
}

#[test]
fn startup_frame_preserves_focused_panel_and_active_workspace() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    assert_eq!(focused_panel_local_id(&app), Some("right-panel"));
    assert_eq!(active_workspace_local_id(&app), Some("right"));

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(focused_panel_local_id(&app), Some("right-panel"));
    assert_eq!(active_workspace_local_id(&app), Some("right"));
}

#[test]
fn startup_frame_leaves_detached_workspace_geometry_unchanged() {
    let runtime_state = RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        detached_workspaces: vec![DetachedWorkspaceState {
            workspace_local_id: "detached".to_string(),
            window: WindowConfig {
                width: 1200.0,
                height: 800.0,
                x: Some(120.0),
                y: Some(80.0),
            },
        }],
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("detached", [500.0, 80.0]),
            editor_workspace_state("right", [900.0, 520.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    let detached_workspace_before = workspace_position(&app, "detached");
    let detached_panel_id = app
        .board
        .panel_id_by_local_id("detached-panel")
        .expect("detached panel");
    let detached_panel_before = app
        .board
        .panel(detached_panel_id)
        .expect("detached panel")
        .layout
        .position;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
    assert_position_near(workspace_position(&app, "detached"), detached_workspace_before);
    assert_position_near(
        app.board
            .panel(detached_panel_id)
            .expect("detached panel")
            .layout
            .position,
        detached_panel_before,
    );
}

#[test]
fn manual_alignment_works_when_startup_is_disabled() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(two_workspace_runtime(Some(CanvasViewState::default()))),
    });
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);
    assert!((workspace_position(&app, "left")[1] - workspace_position(&app, "right")[1]).abs() > 1.0);

    app.execute_command(&ctx, &CommandId::AlignWorkspacesHorizontally);

    assert_horizontal_row(&app, "left", "right");
    assert!(app.pan_target.is_some());
    assert!(app.runtime_dirty_since.is_some());
}

#[test]
fn manual_alignment_still_runs_after_startup_one_shot() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);
    let right_id = app.board.workspace_id_by_local_id("right").expect("right workspace");
    assert!(app.board.translate_workspace(right_id, [0.0, 137.0]));

    app.execute_command(&ctx, &CommandId::AlignWorkspacesHorizontally);

    assert_horizontal_row(&app, "left", "right");
    assert!(app.pan_target.is_some());
}

#[test]
fn already_aligned_startup_does_not_mark_runtime_dirty() {
    let runtime_state = RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![
            editor_workspace_state("left", [100.123, 300.456]),
            editor_workspace_state("right", [700.789, 500.987]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    let workspace_ids: Vec<_> = app.board.workspaces.iter().map(|workspace| workspace.id).collect();
    app.board
        .align_workspaces_horizontally(&workspace_ids)
        .expect("two workspaces");
    app.runtime_dirty_since = None;

    run_frame_at_configured_size(&ctx, &mut app);

    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn startup_organization_waits_for_async_board_restore() {
    let (_temp, ctx, mut app) = enabled_test_app(RuntimeState::default());
    let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
    app.startup_receiver = Some(bootstrap_rx);
    app.theme_applied = true;

    run_app_frame(&ctx, &mut app);
    assert!(app.board.workspaces.is_empty());

    bootstrap_tx
        .send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
            runtime_state: two_workspace_runtime(Some(CanvasViewState::default())),
            session_catalog: AgentSessionCatalog::default(),
            runtime_state_changed: false,
        })))
        .expect("startup bootstrap");
    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn startup_organization_waits_for_session_choice_and_activation() {
    let chooser = StartupChooser {
        reason: StartupPromptReason::MultipleRecoverable,
        config_path: "/tmp/horizon-config.yaml".to_string(),
        sessions: Vec::new(),
    };
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(&enabled_config(), StartupDecision::Choose(chooser));
    app.theme_applied = true;

    run_app_frame(&ctx, &mut app);
    assert!(app.startup_chooser.is_some());
    assert!(app.board.workspaces.is_empty());

    app.activate_ephemeral_session(&two_workspace_runtime(Some(CanvasViewState::default())));
    run_frame_at_configured_size(&ctx, &mut app);

    assert!(app.startup_chooser.is_none());
    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn loading_another_runtime_state_rearms_startup_organization() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);

    let second_runtime = RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![
            editor_workspace_state("second-left", [50.0, 120.0]),
            editor_workspace_state("second-right", [850.0, 640.0]),
        ],
        ..RuntimeState::default()
    };
    app.activate_ephemeral_session(&second_runtime);
    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "second-left", "second-right");
}

#[test]
fn queued_workspace_changes_are_applied_before_startup_organization() {
    let workspace = WorkspaceState {
        local_id: "source".to_string(),
        name: "source".to_string(),
        position: Some([100.0, 300.0]),
        panels: vec![
            editor_panel_state("stay", [120.0, 360.0]),
            editor_panel_state("move", [480.0, 360.0]),
        ],
        ..WorkspaceState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![workspace],
        ..RuntimeState::default()
    });
    app.theme_applied = true;
    let moving_panel = app.board.panel_id_by_local_id("move").expect("moving panel");
    app.workspace_creates.push(moving_panel);

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(app.board.workspaces.len(), 2);
    let first_y = app.board.workspaces[0].position[1];
    let second_y = app.board.workspaces[1].position[1];
    assert!((first_y - second_y).abs() <= POSITION_TOLERANCE);
}

#[test]
fn normalization_removes_empty_workspace_before_startup_organization() {
    let runtime_state = RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![
            WorkspaceState {
                local_id: "empty".to_string(),
                name: "empty".to_string(),
                position: Some([0.0, 900.0]),
                ..WorkspaceState::default()
            },
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [700.0, 500.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert!(app.board.workspace_id_by_local_id("empty").is_none());
    assert_horizontal_row(&app, "left", "right");
    assert!((workspace_position(&app, "left")[1] - 300.0).abs() <= POSITION_TOLERANCE);
}

#[test]
fn restored_root_viewport_defers_layout_and_initial_pan_until_observed() {
    let mut runtime_state = two_workspace_runtime(None);
    runtime_state.window = Some(WindowConfig {
        width: 1400.0,
        height: 900.0,
        x: Some(180.0),
        y: Some(120.0),
    });
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    let stale_input = raw_input([900.0, 700.0], Some([20.0, 30.0]));
    let _ = ctx.run(stale_input.clone(), |ctx| app.restore_window_viewport(ctx));

    run_app_frame_with_input(&ctx, &mut app, stale_input);

    assert!(app.startup_workspace_organization_pending);
    assert!(!app.initial_pan_done);
    assert!((workspace_position(&app, "left")[1] - workspace_position(&app, "right")[1]).abs() > 1.0);
    assert!((app.window_config.width - 1400.0).abs() <= POSITION_TOLERANCE);
    assert!((app.window_config.height - 900.0).abs() <= POSITION_TOLERANCE);

    run_app_frame_with_input(&ctx, &mut app, raw_input([1_100.0, 780.0], Some([100.0, 80.0])));

    assert!(app.pending_root_viewport_restore.is_some());
    assert!(app.startup_workspace_organization_pending);
    assert!(!app.initial_pan_done);

    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], Some([180.0, 120.0])));

    assert!(app.pending_root_viewport_restore.is_none());
    assert_horizontal_row(&app, "left", "right");
    assert!(app.initial_pan_done);
    assert!(app.pan_target.is_none());

    let left_id = app.board.workspace_id_by_local_id("left").expect("left workspace");
    let (min, max) = app.board.workspace_bounds(left_id).expect("left bounds");
    let canvas_rect = app.canvas_rect(&ctx);
    let frame_left = min[0] - WS_BG_PAD;
    let frame_top = min[1] - WS_BG_PAD - WS_TITLE_HEIGHT;
    let frame_bottom = max[1] + WS_BG_PAD;
    let mapped = app.canvas_view.canvas_to_screen(
        [canvas_rect.min.x, canvas_rect.min.y],
        [frame_left, (frame_top + frame_bottom) * 0.5],
    );
    assert!((mapped[0] - (canvas_rect.min.x + 40.0)).abs() <= POSITION_TOLERANCE);
    assert!((mapped[1] - canvas_rect.center().y).abs() <= POSITION_TOLERANCE);
}

#[test]
fn session_switch_does_not_save_before_viewport_and_initial_pan_are_ready() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);
    let old_workspace_count = app.board.workspaces.len();
    let old_panel_count = app.board.panels.len();
    let session = app
        .session_store
        .create_session_from_runtime(RuntimeState {
            window: Some(WindowConfig {
                width: 1400.0,
                height: 900.0,
                x: None,
                y: None,
            }),
            canvas_view: None,
            workspaces: vec![editor_workspace_state("target", [800.0, 500.0])],
            ..RuntimeState::default()
        })
        .expect("target session");
    let runtime_yaml_before = std::fs::read_to_string(&session.runtime_state_path).expect("read target runtime");
    let stale_input = raw_input([900.0, 700.0], None);
    let _ = ctx.run(stale_input.clone(), |ctx| app.activate_runtime_session(ctx, &session));
    let _ = ctx.run(stale_input, |ctx| {
        app.finalize_frame(ctx, false, old_workspace_count, old_panel_count);
    });

    let runtime_yaml_after = std::fs::read_to_string(&session.runtime_state_path).expect("read target runtime");
    assert_eq!(runtime_yaml_after, runtime_yaml_before);
    assert!(!runtime_yaml_after.contains("canvas_view:"));
    assert!(app.pending_root_viewport_restore.is_some());
    assert!(!app.initial_pan_done);
}

#[test]
fn startup_organization_precedes_one_immediate_initial_pan() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    app.theme_applied = true;
    assert_eq!(focused_panel_local_id(&app), Some("right-panel"));

    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
    assert!(app.initial_pan_done);
    assert!(app.pan_target.is_none());
    assert_eq!(focused_panel_local_id(&app), Some("right-panel"));
    assert_eq!(active_workspace_local_id(&app), Some("right"));

    let left_id = app.board.workspace_id_by_local_id("left").expect("left workspace");
    let (min, max) = app.board.workspace_bounds(left_id).expect("left bounds");
    let canvas_rect = app.canvas_rect(&ctx);
    let frame_left = min[0] - WS_BG_PAD;
    let frame_top = min[1] - WS_BG_PAD - WS_TITLE_HEIGHT;
    let frame_bottom = max[1] + WS_BG_PAD;
    let mapped = app.canvas_view.canvas_to_screen(
        [canvas_rect.min.x, canvas_rect.min.y],
        [frame_left, (frame_top + frame_bottom) * 0.5],
    );
    assert!((mapped[0] - (canvas_rect.min.x + 40.0)).abs() <= POSITION_TOLERANCE);
    assert!((mapped[1] - canvas_rect.center().y).abs() <= POSITION_TOLERANCE);

    let settled_view = app.canvas_view;
    run_frame_at_configured_size(&ctx, &mut app);
    assert_eq!(app.canvas_view, settled_view);
    assert!(app.pan_target.is_none());
}

#[test]
fn config_enablement_takes_effect_on_the_next_loaded_session() {
    let runtime_state = two_workspace_runtime(Some(CanvasViewState::default()));
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state.clone()),
    });
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);

    app.apply_runtime_config(&enabled_config());
    run_frame_at_configured_size(&ctx, &mut app);
    assert!((workspace_position(&app, "left")[1] - workspace_position(&app, "right")[1]).abs() > 1.0);

    app.activate_ephemeral_session(&runtime_state);
    run_frame_at_configured_size(&ctx, &mut app);
    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn fewer_than_two_attached_workspaces_are_stable_no_ops() {
    let (_temp, ctx, mut app) = enabled_test_app(RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![editor_workspace_state("only", [100.0, 300.0])],
        ..RuntimeState::default()
    });
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);
    run_frame_at_configured_size(&ctx, &mut app);

    assert_position_near(workspace_position(&app, "only"), [100.0, 300.0]);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn viewport_restore_fallback_is_bounded() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(None));
    app.theme_applied = true;
    let stale_input = raw_input([900.0, 700.0], None);
    let _ = ctx.run(stale_input.clone(), |ctx| app.restore_window_viewport(ctx));

    for _ in 0..=super::super::startup_session::ROOT_VIEWPORT_RESTORE_MAX_WAIT_FRAMES {
        run_app_frame_with_input(&ctx, &mut app, stale_input.clone());
    }

    assert!(app.pending_root_viewport_restore.is_none());
    assert_horizontal_row(&app, "left", "right");
}
