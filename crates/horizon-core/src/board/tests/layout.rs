use crate::layout::{TILE_GAP, WS_INNER_PAD};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelId};

use super::super::arrangement::rects_overlap;
use super::super::*;
use super::{editor_panel_options, shell_panel_options};

#[test]
fn arranging_workspace_records_selected_layout() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn new_workspaces_use_grid_layout_by_default() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");

    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[0] + TILE_GAP,
            origin[1] + WS_INNER_PAD
        ]
    ));
    assert!(vec2_eq(
        board.panel(third).expect("third panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP
        ]
    ));
}

#[test]
fn default_grid_layout_accepts_fifth_panel() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    for index in 0..5 {
        board
            .create_panel(editor_panel_options(), workspace_id)
            .unwrap_or_else(|error| panic!("panel {} should spawn: {error}", index + 1));
    }

    let workspace = board.workspace(workspace_id).expect("workspace");
    assert_eq!(workspace.panels.len(), 5);
    assert_eq!(workspace.layout, Some(WorkspaceLayout::Grid));

    let fifth_panel = board.panel(workspace.panels[4]).expect("fifth panel");
    assert!(vec2_eq(
        fifth_panel.layout.position,
        [
            origin[0] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[0] + TILE_GAP,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
}

#[test]
fn adding_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");

    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
}

#[test]
fn closing_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    board.close_panel(first);

    assert!(vec2_eq(
        board.panel(second).expect("remaining panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn closing_middle_panel_reflows_arranged_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    board.close_panel(second);

    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(third).expect("third panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP,
        ]
    ));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
}

#[test]
fn resizing_rows_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert!(board.resize_panel(first, [640.0, 420.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
    assert!(vec2_eq(first_panel.layout.size, [640.0, 420.0]));
    assert!(vec2_eq(second_panel.layout.size, [640.0, 420.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD + 420.0 + TILE_GAP]
    ));
}

#[test]
fn resizing_columns_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("columns");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Columns);

    assert!(board.resize_panel(first, [700.0, 360.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Columns)
    );
    assert!(vec2_eq(first_panel.layout.size, [700.0, 360.0]));
    assert!(vec2_eq(second_panel.layout.size, [700.0, 360.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD + 700.0 + TILE_GAP, origin[1] + WS_INNER_PAD]
    ));
}

#[test]
fn resizing_grid_layout_reflows_siblings_in_tandem() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("grid");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

    assert!(board.resize_panel(first, [610.0, 390.0]));

    let first_panel = board.panel(first).expect("first panel");
    let second_panel = board.panel(second).expect("second panel");
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Grid)
    );
    assert!(vec2_eq(first_panel.layout.size, [610.0, 390.0]));
    assert!(vec2_eq(second_panel.layout.size, [610.0, 390.0]));
    assert!(vec2_eq(
        second_panel.layout.position,
        [origin[0] + WS_INNER_PAD + 610.0 + TILE_GAP, origin[1] + WS_INNER_PAD]
    ));
}

#[test]
fn adding_panel_preserves_live_rows_panel_size() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let origin = board.workspace(workspace_id).expect("workspace").position;

    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    assert!(board.resize_panel(first, [600.0, 400.0]));

    let third = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("third panel should spawn");

    let second_panel = board.panel(second).expect("second panel");
    let third_panel = board.panel(third).expect("third panel");
    assert!(vec2_eq(second_panel.layout.size, [600.0, 400.0]));
    assert!(vec2_eq(third_panel.layout.size, [600.0, 400.0]));
    assert!(vec2_eq(
        third_panel.layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + 2.0 * (400.0 + TILE_GAP)
        ]
    ));
}

#[test]
fn resizing_rows_layout_pushes_neighbor_workspace_horizontally_when_width_growth_dominates() {
    let mut board = Board::new();
    let rows = board.create_workspace_at("rows", [0.0, 40.0]);
    let beta = board.create_workspace_at("beta", [630.0, 40.0]);

    let first = board
        .create_panel(editor_panel_options(), rows)
        .expect("first panel should spawn");
    board.arrange_workspace(rows, WorkspaceLayout::Rows);
    board
        .create_panel(editor_panel_options(), rows)
        .expect("second panel should spawn");
    board
        .create_panel(editor_panel_options(), beta)
        .expect("beta panel should spawn");

    let beta_before = board.workspace(beta).expect("beta workspace").position;

    assert!(board.resize_panel(first, [640.0, 420.0]));

    let beta_after = board.workspace(beta).expect("beta workspace").position;
    assert!(
        beta_after[0] > beta_before[0],
        "expected beta to move right from {beta_before:?}, got {beta_after:?}"
    );
    assert!(
        (beta_after[1] - beta_before[1]).abs() <= f32::EPSILON,
        "expected beta y to stay at {}, got {}",
        beta_before[1],
        beta_after[1],
    );
}

#[test]
fn manual_panel_move_returns_workspace_to_freeform() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    assert!(board.move_panel(panel_id, [420.0, 360.0]));

    assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
}

#[test]
fn clearing_workspace_layout_preserves_current_panel_positions() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("rows");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let arranged_position = board.panel(panel_id).expect("panel").layout.position;

    assert!(board.clear_workspace_layout(workspace_id));

    assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);
    let current_position = board.panel(panel_id).expect("panel").layout.position;
    assert!(
        vec2_eq(current_position, arranged_position),
        "expected {arranged_position:?}, got {current_position:?}"
    );
}

#[test]
fn switching_from_manual_to_preset_arranges_immediately() {
    for layout in WorkspaceLayout::ALL {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("manual");

        let first = board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("first panel should spawn");
        let second = board
            .create_panel(editor_panel_options(), workspace_id)
            .expect("second panel should spawn");

        // Placing a panel manually returns the workspace to freeform. The
        // second panel sits far enough out that the fitted sizes below stay
        // above the default-size clamp on every axis.
        assert!(board.move_panel(first, [180.0, 140.0]));
        assert!(board.move_panel(second, [700.0, 900.0]));
        assert_eq!(board.workspace(workspace_id).expect("workspace").layout, None);

        let origin = board.workspace(workspace_id).expect("workspace").position;
        let content_min = [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD];
        let mut content = [0.0f32; 2];
        for id in [first, second] {
            let panel = board.panel(id).expect("panel");
            content[0] = content[0].max(panel.layout.position[0] + panel.layout.size[0] - content_min[0]);
            content[1] = content[1].max(panel.layout.position[1] + panel.layout.size[1] - content_min[1]);
        }
        // Two panels: Rows stacks them, Columns and Grid sit them side by side.
        let expected_size = match layout {
            WorkspaceLayout::Rows => [content[0], (content[1] - TILE_GAP) / 2.0],
            WorkspaceLayout::Columns | WorkspaceLayout::Grid => [(content[0] - TILE_GAP) / 2.0, content[1]],
        };
        assert!(
            expected_size[0] > DEFAULT_PANEL_SIZE[0] && expected_size[1] > DEFAULT_PANEL_SIZE[1],
            "test setup must keep fitted sizes above the default-size clamp, got {expected_size:?}"
        );

        board.arrange_workspace(workspace_id, layout);

        let workspace = board.workspace(workspace_id).expect("workspace");
        assert_eq!(workspace.layout, Some(layout));
        let first_panel = board.panel(first).expect("first panel");
        let second_panel = board.panel(second).expect("second panel");
        for (name, panel) in [("first", first_panel), ("second", second_panel)] {
            assert!(
                vec2_eq(panel.layout.size, expected_size),
                "{layout:?} should fit the {name} panel to the content area as {expected_size:?}, got {:?}",
                panel.layout.size
            );
        }
        assert!(
            vec2_eq(first_panel.layout.position, content_min),
            "{layout:?} should anchor the first panel at the workspace origin, got {:?}",
            first_panel.layout.position
        );
        let expected_second = match layout {
            WorkspaceLayout::Rows => [content_min[0], content_min[1] + expected_size[1] + TILE_GAP],
            WorkspaceLayout::Columns | WorkspaceLayout::Grid => {
                [content_min[0] + expected_size[0] + TILE_GAP, content_min[1]]
            }
        };
        assert!(
            vec2_eq(second_panel.layout.position, expected_second),
            "{layout:?} should place the second panel at {expected_second:?}, got {:?}",
            second_panel.layout.position
        );
    }
}

#[test]
fn arranging_preset_pushes_neighbor_workspace_on_frame_growth() {
    let mut board = Board::new();
    let left = board.create_workspace_at("left", [0.0, 40.0]);

    let first = board
        .create_panel(editor_panel_options(), left)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), left)
        .expect("second panel should spawn");

    // Stack both panels on the same spot: the content area is one panel wide,
    // so the fitted Columns width clamps to the default size and the
    // arrangement grows the frame toward the neighbor.
    assert!(board.move_panel(first, [40.0, 80.0]));
    assert!(board.move_panel(second, [40.0, 80.0]));

    let right = board.create_workspace_at("right", [630.0, 40.0]);
    board
        .create_panel(editor_panel_options(), right)
        .expect("neighbor panel should spawn");

    let right_before = board.workspace(right).expect("right workspace").position;
    assert!(
        vec2_eq(right_before, [630.0, 40.0]),
        "test setup must leave the neighbor at its explicit position, got {right_before:?}"
    );

    board.arrange_workspace(left, WorkspaceLayout::Columns);

    let right_after = board.workspace(right).expect("right workspace").position;
    assert!(
        right_after[0] > right_before[0],
        "expected the neighbor to be pushed right from {right_before:?}, got {right_after:?}"
    );
    assert!(
        (right_after[1] - right_before[1]).abs() <= f32::EPSILON,
        "expected the neighbor y to stay at {}, got {}",
        right_before[1],
        right_after[1],
    );
}

#[test]
fn switching_between_presets_still_rearranges() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("switch");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let workspace = board.workspace(workspace_id).expect("workspace");
    assert_eq!(workspace.layout, Some(WorkspaceLayout::Rows));
    let origin = workspace.position;
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [origin[0] + WS_INNER_PAD, origin[1] + WS_INNER_PAD]
    ));
    assert!(vec2_eq(
        board.panel(second).expect("second panel").layout.position,
        [
            origin[0] + WS_INNER_PAD,
            origin[1] + WS_INNER_PAD + DEFAULT_PANEL_SIZE[1] + TILE_GAP
        ]
    ));
}

#[test]
fn hidden_panels_leave_layout_and_focus_and_showing_focuses_them() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("visibility");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);
    assert_eq!(board.focused, Some(second));

    assert!(board.set_panel_visible(second, false));
    assert!(!board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(first));
    assert!(vec2_eq(
        board.panel(first).expect("first panel").layout.position,
        [
            board.workspace(workspace_id).expect("workspace").position[0] + WS_INNER_PAD,
            board.workspace(workspace_id).expect("workspace").position[1] + WS_INNER_PAD,
        ],
    ));

    assert!(board.set_panel_visible(second, true));
    assert!(board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(second));
    assert!(board.workspace_bounds(workspace_id).is_some());

    assert!(board.set_panel_visible(second, false));
    board.focus(second);
    assert!(board.panel(second).expect("second panel").visible);
    assert_eq!(board.focused, Some(second));
}

/// Two terminals and an agent panel, placed where a config, a saved session or
/// a person's drag puts them: not on the default tile grid.
fn off_grid_workspace(board: &mut Board) -> (WorkspaceId, PanelId) {
    let workspace = board.create_workspace_at("off the grid", [0.0, 0.0]);
    for (position, size) in [
        ([40.0, 60.0], [550.0, 420.0]),
        ([630.0, 60.0], [550.0, 420.0]),
        ([1220.0, 60.0], [330.0, 440.0]),
    ] {
        board
            .create_panel(
                PanelOptions {
                    position: Some(position),
                    size: Some(size),
                    ..editor_panel_options()
                },
                workspace,
            )
            .expect("a placed panel spawns");
    }
    let agent = board.workspace(workspace).expect("workspace").panels[2];
    (workspace, agent)
}

/// A terminal panel, as `open_terminal` places one: the default panel size.
fn terminal_panel_options() -> PanelOptions {
    shell_panel_options()
}

/// A card panel, as the block request path creates one.
fn card_panel_options(size: [f32; 2]) -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Block,
        size: Some(size),
        ..PanelOptions::default()
    }
}

/// Every pair of panels of a workspace whose rectangles overlap.
fn overlapping_panels(board: &Board, workspace: WorkspaceId) -> Vec<(PanelId, PanelId)> {
    let panels: Vec<(PanelId, [f32; 4])> = board
        .workspace(workspace)
        .expect("workspace")
        .panels
        .iter()
        .filter_map(|id| board.panel(*id))
        .map(|panel| {
            let (position, size) = (panel.layout.position, panel.layout.size);
            (
                panel.id,
                [position[0], position[1], position[0] + size[0], position[1] + size[1]],
            )
        })
        .collect();
    let mut pairs = Vec::new();
    for (index, (left_id, left)) in panels.iter().enumerate() {
        for (right_id, right) in &panels[index + 1..] {
            if rects_overlap(*left, *right) {
                pairs.push((*left_id, *right_id));
            }
        }
    }
    pairs
}

#[test]
fn a_new_panel_takes_a_free_place_beside_off_grid_panels() {
    let mut board = Board::new();
    let (workspace, _agent) = off_grid_workspace(&mut board);

    board
        .create_panel(card_panel_options([420.0, 300.0]), workspace)
        .expect("the new panel spawns");

    assert_eq!(
        overlapping_panels(&board, workspace),
        Vec::new(),
        "a panel that is placed by a config keeps its room"
    );
}

#[test]
fn three_cards_created_in_a_row_cover_no_panel() {
    let mut board = Board::new();
    let (workspace, agent) = off_grid_workspace(&mut board);

    let mut cards = Vec::new();
    for _ in 0..3 {
        let beside = cards.last().copied().unwrap_or(agent);
        let card = board
            .create_panel_beside(card_panel_options([420.0, 500.0]), workspace, beside)
            .expect("the card spawns");
        cards.push(card);
    }

    assert_eq!(
        overlapping_panels(&board, workspace),
        Vec::new(),
        "no card covers a panel"
    );
    let agent = board.panel(agent).expect("agent panel");
    let first = board.panel(cards[0]).expect("first card");
    assert!(
        first.layout.position[0] >= agent.layout.position[0] + agent.layout.size[0]
            || first.layout.position[1] >= agent.layout.position[1] + agent.layout.size[1],
        "the first card takes a free place beside the agent panel: {:?} against {:?}",
        first.layout.position,
        agent.layout.position
    );
}

#[test]
fn a_click_on_empty_canvas_keeps_its_exact_point() {
    let mut board = Board::new();
    let (workspace, _) = off_grid_workspace(&mut board);
    let point = [1660.0, 620.0];

    assert_eq!(
        board.panel_place_at(workspace, point, DEFAULT_PANEL_SIZE),
        point,
        "a place that covers no panel is the place the person clicked"
    );
}

#[test]
fn a_click_that_would_cover_a_panel_takes_the_nearest_free_place() {
    let mut board = Board::new();
    let (workspace, _) = off_grid_workspace(&mut board);
    let covered = board
        .workspace(workspace)
        .expect("workspace")
        .panels
        .first()
        .copied()
        .expect("a panel of the workspace");
    let corner = board.panel(covered).expect("the panel").layout.position;

    let place = board.panel_place_at(workspace, corner, DEFAULT_PANEL_SIZE);

    assert_ne!(place, corner, "a place on top of a panel is not a place");
    let candidate = [
        place[0],
        place[1],
        place[0] + DEFAULT_PANEL_SIZE[0],
        place[1] + DEFAULT_PANEL_SIZE[1],
    ];
    let overlaps = board
        .workspace(workspace)
        .expect("workspace")
        .panels
        .iter()
        .filter_map(|id| board.panel(*id))
        .any(|panel| {
            let (position, size) = (panel.layout.position, panel.layout.size);
            rects_overlap(
                candidate,
                [position[0], position[1], position[0] + size[0], position[1] + size[1]],
            )
        });
    assert!(!overlaps, "the place covers no panel of the workspace: {place:?}");
}

/// The rectangle `[min_x, min_y, max_x, max_y]` of a panel.
fn panel_bounds(board: &Board, id: PanelId) -> [f32; 4] {
    let panel = board.panel(id).expect("panel");
    let [x, y] = panel.layout.position;
    let [width, height] = panel.layout.size;
    [x, y, x + width, y + height]
}

#[test]
fn a_card_beside_its_agent_panel_aligns_its_top_edge() {
    let mut board = Board::new();
    let (workspace, agent) = off_grid_workspace(&mut board);

    let card = board
        .create_panel_beside(card_panel_options([420.0, 500.0]), workspace, agent)
        .expect("the card spawns");

    let agent = panel_bounds(&board, agent);
    let card = panel_bounds(&board, card);
    assert_eq!(card[1], agent[1], "the top edges are level: {card:?} against {agent:?}");
    assert_eq!(
        card[0],
        agent[2] + TILE_GAP,
        "the card starts at the right edge of the agent"
    );
    assert_eq!(overlapping_panels(&board, workspace), Vec::new(), "no panel is covered");
}

#[test]
fn a_card_below_its_agent_panel_aligns_its_left_edge() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("below", [0.0, 0.0]);
    let agent = board
        .create_panel(
            PanelOptions {
                position: Some([1220.0, 60.0]),
                size: Some([330.0, 440.0]),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("the agent panel spawns");
    // A panel where a card would stand beside the agent, off the tile grid, so
    // the next card goes below.
    board
        .create_panel(
            PanelOptions {
                position: Some([1570.0, 60.0]),
                size: Some([420.0, 400.0]),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("the neighbouring panel spawns");

    let card = board
        .create_panel_beside(card_panel_options([420.0, 500.0]), workspace, agent)
        .expect("the card spawns");

    let agent = panel_bounds(&board, agent);
    let card = panel_bounds(&board, card);
    assert_eq!(
        card[0], agent[0],
        "the left edges are level: {card:?} against {agent:?}"
    );
    assert_eq!(card[1], agent[3] + TILE_GAP, "the card starts below the agent");
    assert_eq!(overlapping_panels(&board, workspace), Vec::new(), "no panel is covered");
}

#[test]
fn a_card_that_fits_in_neither_place_beside_its_agent_continues_its_column() {
    let mut board = Board::new();
    let workspace = board.create_workspace_at("crowded", [0.0, 0.0]);
    let agent = board
        .create_panel(
            PanelOptions {
                position: Some([1220.0, 60.0]),
                size: Some([330.0, 440.0]),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("the agent panel spawns");
    // A panel beside the agent and one below it take both places a card that
    // belongs to the agent would take.
    for (position, size) in [([1570.0, 60.0], [420.0, 400.0]), ([1220.0, 520.0], [420.0, 500.0])] {
        board
            .create_panel(
                PanelOptions {
                    position: Some(position),
                    size: Some(size),
                    ..editor_panel_options()
                },
                workspace,
            )
            .expect("the neighbouring panel spawns");
    }

    let card = board
        .create_panel_beside(card_panel_options([420.0, 500.0]), workspace, agent)
        .expect("the card spawns");

    assert_eq!(
        overlapping_panels(&board, workspace),
        Vec::new(),
        "a crowded workspace still gives the card a free place"
    );
    let agent = panel_bounds(&board, agent);
    let card = panel_bounds(&board, card);
    assert_ne!(
        (card[0], card[1]),
        (agent[2] + TILE_GAP, agent[1]),
        "the place beside the agent is taken: {card:?}"
    );
    assert_eq!(
        card[0], agent[0],
        "the card continues the column of the agent: {card:?} against {agent:?}"
    );
    assert_eq!(
        card[1],
        520.0 + 500.0 + TILE_GAP,
        "the card stands directly below the panel that took the place under the agent"
    );
}

#[test]
fn two_terminals_opened_by_an_agent_keep_its_column_below_two_cards() {
    let mut board = Board::new();
    let (workspace, agent) = off_grid_workspace(&mut board);

    // The two cards of the agent take the place beside its panel.
    let mut card = agent;
    for _ in 0..2 {
        card = board
            .create_panel_beside(card_panel_options([420.0, 300.0]), workspace, card)
            .expect("the card spawns");
    }

    // The SSH terminals the agent opens for the person belong to the agent
    // panel, so both places beside it are taken by the cards.
    let mut terminals = Vec::new();
    for _ in 0..2 {
        terminals.push(
            board
                .create_panel_beside(terminal_panel_options(), workspace, agent)
                .expect("the terminal spawns"),
        );
    }

    let agent = panel_bounds(&board, agent);
    let first = panel_bounds(&board, terminals[0]);
    let second = panel_bounds(&board, terminals[1]);
    assert_eq!(
        first[0], agent[0],
        "the first terminal keeps the left edge of the agent panel: {first:?} against {agent:?}"
    );
    assert_eq!(
        second[0], agent[0],
        "the second terminal keeps the left edge of the agent panel: {second:?} against {agent:?}"
    );
    assert_eq!(
        first[1],
        agent[3] + TILE_GAP,
        "the first terminal stands directly below the agent panel"
    );
    assert_eq!(
        second[1],
        first[3] + TILE_GAP,
        "the second terminal stands directly below the first"
    );
    assert_eq!(overlapping_panels(&board, workspace), Vec::new(), "no panel is covered");
}
