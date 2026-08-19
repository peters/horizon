use super::*;
use crate::test_egui::DiscardTextures;

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
    assert_workspace_center_is_on_canvas(&ctx, &app, "right");
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
fn startup_frame_does_not_treat_a_workspace_sliver_as_visible() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    let size = [app.window_config.width, app.window_config.height];
    let _ = ctx
        .run_ui(raw_input(size, None), |ui| {
            let (pos, frame_size) = workspace_frame(&app, "right");
            let canvas_rect = app.canvas_rect(ui);
            app.canvas_view.set_pan_offset([
                1.0 - (pos.x + frame_size.x),
                canvas_rect.center().y - canvas_rect.min.y - (pos.y + frame_size.y * 0.5),
            ]);
        })
        .discard_textures();
    let sliver_view = app.canvas_view;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
    assert_workspace_center_is_on_canvas(&ctx, &app, "left");
    assert_ne!(app.canvas_view, sliver_view);
    assert!(app.pan_target.is_none());
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
fn startup_frame_preserves_persisted_active_workspace_without_persisted_focus() {
    let runtime_state = RuntimeState {
        active_workspace_local_id: Some("right".to_string()),
        focused_panel_local_id: None,
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            editor_workspace_state("right", [700.0, 500.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    assert_eq!(active_workspace_local_id(&app), Some("right"));
    assert_eq!(focused_panel_local_id(&app), Some("left-panel"));

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(active_workspace_local_id(&app), Some("right"));
    assert_eq!(focused_panel_local_id(&app), Some("left-panel"));
    assert_horizontal_row(&app, "left", "right");
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
fn manual_alignment_is_a_clean_no_op_when_row_and_view_are_already_aligned() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(two_workspace_runtime(Some(CanvasViewState::default()))),
    });
    app.theme_applied = true;
    let workspace_ids: Vec<_> = app.board.workspaces.iter().map(|workspace| workspace.id).collect();
    let alignment = app
        .board
        .align_workspaces_horizontally(&workspace_ids)
        .expect("two workspaces");
    let (min, max) = app
        .board
        .workspace_bounds(alignment.leftmost_workspace)
        .expect("leftmost bounds");
    app.focus_workspace_bounds(&ctx, min, max, true);
    let target = app.pan_target.take().expect("manual alignment target");
    app.canvas_view.set_pan_offset([target.x, target.y]);
    app.runtime_dirty_since = None;

    app.execute_command(&ctx, &CommandId::AlignWorkspacesHorizontally);

    assert!(app.pan_target.is_none());
    assert!(app.runtime_dirty_since.is_none());
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
    let (min, _max) = app.board.workspace_bounds(left_id).expect("left bounds");
    let canvas_rect = app.canvas_rect(&ctx);
    let frame_left = min[0] - WS_BG_PAD;
    let frame_top = min[1] - WS_BG_PAD - WS_TITLE_HEIGHT;
    let mapped = app
        .canvas_view
        .canvas_to_screen([canvas_rect.min.x, canvas_rect.min.y], [frame_left, frame_top]);
    assert!((mapped[0] - (canvas_rect.min.x + 40.0)).abs() <= POSITION_TOLERANCE);
    assert!((mapped[1] - canvas_rect.center().y).abs() <= POSITION_TOLERANCE);

    let settled_view = app.canvas_view;
    run_frame_at_configured_size(&ctx, &mut app);
    assert_eq!(app.canvas_view, settled_view);
    assert!(app.pan_target.is_none());
}

#[test]
fn initial_pan_uses_the_row_head_selected_by_core_alignment() {
    let runtime_state = RuntimeState {
        canvas_view: None,
        active_workspace_local_id: Some("origin-left".to_string()),
        focused_panel_local_id: Some("origin-left-panel".to_string()),
        workspaces: vec![
            WorkspaceState {
                local_id: "origin-left".to_string(),
                name: "origin-left".to_string(),
                position: Some([100.0, 300.0]),
                panels: vec![editor_panel_state("origin-left-panel", [120.0, 360.0])],
                ..WorkspaceState::default()
            },
            WorkspaceState {
                local_id: "frame-left".to_string(),
                name: "frame-left".to_string(),
                position: Some([500.0, 500.0]),
                panels: vec![editor_panel_state("frame-left-panel", [520.0, 560.0])],
                ..WorkspaceState::default()
            },
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;
    let origin_left_id = app
        .board
        .workspace_id_by_local_id("origin-left")
        .expect("origin-left workspace");
    let frame_left_id = app
        .board
        .workspace_id_by_local_id("frame-left")
        .expect("frame-left workspace");
    let origin_left_panel = app
        .board
        .panel_id_by_local_id("origin-left-panel")
        .expect("origin-left panel");
    let frame_left_panel = app
        .board
        .panel_id_by_local_id("frame-left-panel")
        .expect("frame-left panel");
    app.board
        .panel_mut(origin_left_panel)
        .expect("origin-left panel")
        .move_to([1_000.0, 360.0]);
    app.board
        .panel_mut(frame_left_panel)
        .expect("frame-left panel")
        .move_to([0.0, 560.0]);
    assert!(
        app.board.workspace_bounds(frame_left_id).expect("frame-left bounds").0[0]
            < app
                .board
                .workspace_bounds(origin_left_id)
                .expect("origin-left bounds")
                .0[0],
        "the fixture must put frame-left's visual frame before origin-left's: frame-left={:?}, origin-left={:?}",
        app.board.workspace_bounds(frame_left_id),
        app.board.workspace_bounds(origin_left_id),
    );

    run_frame_at_configured_size(&ctx, &mut app);

    let (pos, _size) = workspace_frame(&app, "frame-left");
    let (origin_left_pos, _size) = workspace_frame(&app, "origin-left");
    assert!(pos.x < origin_left_pos.x);
    assert!((pos.y - origin_left_pos.y).abs() <= POSITION_TOLERANCE);
    let canvas_rect = app.canvas_rect(&ctx);
    let mapped = app.canvas_to_screen(canvas_rect, pos);
    let origin_left_frame = workspace_frame(&app, "origin-left");
    let origin_left_mapped = app.canvas_to_screen(canvas_rect, origin_left_frame.0);
    assert!(
        (mapped.x - (canvas_rect.min.x + 40.0)).abs() <= POSITION_TOLERANCE,
        "expected the core-selected frame-left row head x anchor {}, got {}; origin-left maps to {}; positions: frame-left={:?}, origin-left={:?}",
        canvas_rect.min.x + 40.0,
        mapped.x,
        origin_left_mapped.x,
        workspace_position(&app, "frame-left"),
        workspace_position(&app, "origin-left"),
    );
    assert!(
        (mapped.y - canvas_rect.center().y).abs() <= POSITION_TOLERANCE,
        "expected frame-left y anchor {}, got {}",
        canvas_rect.center().y,
        mapped.y
    );
    assert_eq!(focused_panel_local_id(&app), Some("origin-left-panel"));
    assert_eq!(active_workspace_local_id(&app), Some("origin-left"));
}

#[test]
fn default_disabled_initial_pan_keeps_focus_and_camera_on_leftmost_workspace() {
    let runtime_state = RuntimeState {
        canvas_view: None,
        workspaces: vec![
            editor_workspace_state("right", [900.0, 80.0]),
            editor_workspace_state("left", [100.0, 300.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state),
    });
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(active_workspace_local_id(&app), Some("left"));
    assert_eq!(focused_panel_local_id(&app), Some("left-panel"));
    let left_id = app.board.workspace_id_by_local_id("left").expect("left workspace");
    let (min, _max) = app.board.workspace_bounds(left_id).expect("left bounds");
    let canvas_rect = app.canvas_rect(&ctx);
    let frame_left = min[0] - WS_BG_PAD;
    let frame_top = min[1] - WS_BG_PAD - WS_TITLE_HEIGHT;
    let mapped = app
        .canvas_view
        .canvas_to_screen([canvas_rect.min.x, canvas_rect.min.y], [frame_left, frame_top]);
    assert!((mapped[0] - (canvas_rect.min.x + 40.0)).abs() <= POSITION_TOLERANCE);
    assert!((mapped[1] - canvas_rect.center().y).abs() <= POSITION_TOLERANCE);
}

#[test]
fn enabled_initial_pan_repairs_an_unpersisted_fallback_selection() {
    let runtime_state = RuntimeState {
        canvas_view: None,
        workspaces: vec![
            editor_workspace_state("right", [900.0, 80.0]),
            editor_workspace_state("left", [100.0, 300.0]),
        ],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(runtime_state);
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_eq!(active_workspace_local_id(&app), Some("left"));
    assert_eq!(focused_panel_local_id(&app), Some("left-panel"));
    assert_workspace_center_is_on_canvas(&ctx, &app, "left");
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
    let later = app.board.create_workspace_at("later", [900.0, 700.0]);
    app.board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                ..horizon_core::PanelOptions::default()
            },
            later,
        )
        .expect("later editor panel");
    let later_position = app.board.workspace(later).expect("later workspace").position;
    run_frame_at_configured_size(&ctx, &mut app);

    assert_position_near(workspace_position(&app, "only"), [100.0, 300.0]);
    let current_later_position = app.board.workspace(later).expect("later workspace").position;
    assert_position_near(current_later_position, later_position);
    assert!(
        (workspace_position(&app, "only")[1] - current_later_position[1]).abs() > 1.0,
        "the one-shot must not rearm merely because a second workspace appears later"
    );
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn non_finite_startup_geometry_does_not_move_neighbors_or_mark_dirty() {
    let invalid_workspace = WorkspaceState {
        local_id: "invalid".to_string(),
        name: "invalid".to_string(),
        position: Some([500.0, 400.0]),
        panels: vec![editor_panel_state("invalid-panel", [f32::NAN, 460.0])],
        ..WorkspaceState::default()
    };
    let (_temp, ctx, mut app) = enabled_test_app(RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![
            editor_workspace_state("left", [100.0, 300.0]),
            invalid_workspace,
            editor_workspace_state("right", [900.0, 700.0]),
        ],
        ..RuntimeState::default()
    });
    let left_before = workspace_position(&app, "left");
    let right_before = workspace_position(&app, "right");
    app.runtime_dirty_since = None;

    let alignment = app.apply_startup_workspace_organization(&ctx);

    assert!(alignment.is_none());
    assert_eq!(
        workspace_position(&app, "left").map(f32::to_bits),
        left_before.map(f32::to_bits)
    );
    assert_eq!(
        workspace_position(&app, "right").map(f32::to_bits),
        right_before.map(f32::to_bits)
    );
    assert!(app.runtime_dirty_since.is_none());
}
