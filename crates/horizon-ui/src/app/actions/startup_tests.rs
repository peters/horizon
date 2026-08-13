use egui::Context;
use horizon_core::{
    AgentSessionCatalog, Board, CanvasViewState, Config, HorizonHome, PanelId, PanelKind, PanelOptions, PanelState,
    RuntimeState, SessionStore, StartupChooser, StartupDecision, StartupPromptReason, WindowConfig, WorkspaceId,
    WorkspaceState,
};

use super::{DetachedWorkspaceViewportState, HorizonApp};
use crate::app::session::{StartupBootstrap, StartupBootstrapOutcome};
use crate::command_registry::CommandId;
use crate::input;

fn test_app() -> (tempfile::TempDir, Context, HorizonApp) {
    test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    })
}

fn test_app_with_startup(startup: StartupDecision) -> (tempfile::TempDir, Context, HorizonApp) {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.yaml");
    let session_store = SessionStore::new(
        HorizonHome::from_root(temp.path().join(".horizon")),
        config_path.clone(),
    );
    let config = Config::default();
    let ctx = Context::default();
    let app = HorizonApp::new_with_egui_context(
        &ctx,
        &config,
        config_path,
        session_store,
        startup,
        input::ObservedKeyboardInputs::default(),
    );
    (temp, ctx, app)
}

fn add_editor_workspace(board: &mut Board, name: &str, position: [f32; 2]) -> (WorkspaceId, PanelId) {
    let workspace_id = board.create_workspace(name);
    assert!(board.move_workspace(workspace_id, position));
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                position: Some([position[0] + 20.0, position[1] + 60.0]),
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("editor panel");
    (workspace_id, panel_id)
}

fn editor_workspace_state(local_id: &str, position: [f32; 2]) -> WorkspaceState {
    WorkspaceState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        position: Some(position),
        panels: vec![PanelState {
            local_id: format!("{local_id}-panel"),
            name: format!("{local_id} notes"),
            kind: PanelKind::Editor,
            position: Some([position[0] + 20.0, position[1] + 60.0]),
            size: Some([320.0, 220.0]),
            ..PanelState::default()
        }],
        ..WorkspaceState::default()
    }
}

fn run_frame(ctx: &Context, app: &mut HorizonApp) {
    let mut frame = eframe::Frame::_new_kittest();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        eframe::App::update(app, ctx, &mut frame);
    });
}

fn positions_match(left: [f32; 2], right: [f32; 2]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left_component, right_component)| (left_component - right_component).abs() <= f32::EPSILON)
}

#[test]
fn startup_organization_runs_once_and_keeps_detached_and_focus_state() {
    let (_temp, ctx, mut app) = test_app();
    let (left, _) = add_editor_workspace(&mut app.board, "left", [100.0, 300.0]);
    let (detached, detached_panel) = add_editor_workspace(&mut app.board, "detached", [600.0, 50.0]);
    let (right, right_panel) = add_editor_workspace(&mut app.board, "right", [1_100.0, 500.0]);
    let detached_local_id = app
        .board
        .workspace(detached)
        .expect("detached workspace")
        .local_id
        .clone();
    app.detached_workspaces.insert(
        detached_local_id,
        DetachedWorkspaceViewportState::new(WindowConfig::default()),
    );
    app.board.focus(right_panel);

    let detached_workspace_position = app.board.workspace(detached).expect("detached workspace").position;
    let detached_panel_position = app.board.panel(detached_panel).expect("detached panel").layout.position;
    let focused_before = app.board.focused;
    let active_before = app.board.active_workspace;

    app.apply_startup_workspace_organization(&ctx);

    let left_position = app.board.workspace(left).expect("left workspace").position;
    let right_position = app.board.workspace(right).expect("right workspace").position;
    assert!((left_position[1] - right_position[1]).abs() <= f32::EPSILON);
    assert!(right_position[0] > left_position[0]);
    assert!(positions_match(
        app.board.workspace(detached).expect("detached workspace").position,
        detached_workspace_position,
    ));
    assert!(positions_match(
        app.board.panel(detached_panel).expect("detached panel").layout.position,
        detached_panel_position,
    ));
    assert_eq!(app.board.focused, focused_before);
    assert_eq!(app.board.active_workspace, active_before);
    assert!(!app.startup_workspace_organization_pending);
    assert!(app.runtime_dirty_since.is_some());

    assert!(app.board.translate_workspace(right, [0.0, 137.0]));
    let manually_moved_position = app.board.workspace(right).expect("right workspace").position;
    app.apply_startup_workspace_organization(&ctx);
    assert!(
        positions_match(
            app.board.workspace(right).expect("right workspace").position,
            manually_moved_position,
        ),
        "startup organization must not become a continuous layout constraint"
    );

    app.execute_command(&ctx, &CommandId::AlignWorkspacesHorizontally);
    let realigned_left = app.board.workspace(left).expect("left workspace").position;
    let realigned_right = app.board.workspace(right).expect("right workspace").position;
    assert!((realigned_left[1] - realigned_right[1]).abs() <= f32::EPSILON);
    assert_eq!(app.board.focused, focused_before);
    assert_eq!(app.board.active_workspace, active_before);
}

#[test]
fn startup_organization_consumes_no_op() {
    let (_temp, ctx, mut app) = test_app();
    let _ = add_editor_workspace(&mut app.board, "only", [100.0, 300.0]);

    app.apply_startup_workspace_organization(&ctx);
    app.apply_startup_workspace_organization(&ctx);

    assert!(!app.startup_workspace_organization_pending);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn aligning_empty_workspaces_still_marks_runtime_state_dirty() {
    let (_temp, ctx, mut app) = test_app();
    let left = app.board.create_workspace("left");
    let right = app.board.create_workspace("right");
    assert!(app.board.move_workspace(left, [100.0, 300.0]));
    assert!(app.board.move_workspace(right, [700.0, 500.0]));

    assert!(app.align_attached_workspaces_horizontally(&ctx));

    assert!(app.runtime_dirty_since.is_some());
    let left_position = app.board.workspace(left).expect("left workspace").position;
    let right_position = app.board.workspace(right).expect("right workspace").position;
    assert!((left_position[1] - right_position[1]).abs() <= f32::EPSILON);
}

#[test]
fn startup_organization_waits_for_async_board_restore() {
    let (_temp, ctx, mut app) = test_app();
    let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
    app.startup_receiver = Some(bootstrap_rx);
    app.theme_applied = true;

    run_frame(&ctx, &mut app);

    assert!(app.startup_workspace_organization_pending);
    assert!(app.board.workspaces.is_empty());

    bootstrap_tx
        .send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
            runtime_state: RuntimeState {
                workspaces: vec![
                    editor_workspace_state("left", [100.0, 300.0]),
                    editor_workspace_state("right", [700.0, 500.0]),
                ],
                ..RuntimeState::default()
            },
            session_catalog: AgentSessionCatalog::default(),
            runtime_state_changed: false,
        })))
        .expect("startup bootstrap");
    run_frame(&ctx, &mut app);

    assert!(!app.startup_workspace_organization_pending);
    let left_position = app.board.workspaces[0].position;
    let right_position = app.board.workspaces[1].position;
    assert!((left_position[1] - right_position[1]).abs() <= f32::EPSILON);
}

#[test]
fn startup_organization_runs_with_persisted_canvas_view() {
    let runtime_state = RuntimeState {
        canvas_view: Some(CanvasViewState::new([220.0, -80.0], 1.25)),
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [700.0, 500.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state),
    });
    app.theme_applied = true;

    assert!(app.initial_pan_done);
    assert!(app.startup_workspace_organization_pending);
    run_frame(&ctx, &mut app);

    assert!(!app.startup_workspace_organization_pending);
    let left_y = app.board.workspaces[0].position[1];
    let right_y = app.board.workspaces[1].position[1];
    assert!((left_y - right_y).abs() <= f32::EPSILON);
}

#[test]
fn startup_organization_waits_for_session_choice() {
    let chooser = StartupChooser {
        reason: StartupPromptReason::MultipleRecoverable,
        config_path: "/tmp/horizon-config.yaml".to_string(),
        sessions: Vec::new(),
    };
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Choose(chooser));
    app.theme_applied = true;

    run_frame(&ctx, &mut app);

    assert!(app.startup_workspace_organization_pending);
    assert!(app.startup_chooser.is_some());
    assert!(app.board.workspaces.is_empty());

    app.activate_ephemeral_session(&RuntimeState {
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [700.0, 500.0]),
        ],
        ..RuntimeState::default()
    });
    run_frame(&ctx, &mut app);

    assert!(!app.startup_workspace_organization_pending);
    assert!(app.startup_chooser.is_none());
    let left_y = app.board.workspaces[0].position[1];
    let right_y = app.board.workspaces[1].position[1];
    assert!((left_y - right_y).abs() <= f32::EPSILON);
}
