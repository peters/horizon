use std::path::PathBuf;

use crate::config::WorkspaceConfig;
use crate::layout::{TILE_GAP, WS_INNER_PAD};
use crate::panel::{DEFAULT_PANEL_SIZE, PanelKind, PanelOptions, PanelResume};
use crate::runtime_state::{PanelState, RuntimeState, WorkspaceState, WorkspaceTemplateRef};
use crate::ssh::{SshConnection, SshConnectionStatus};

use super::super::*;
use super::editor_panel_options;

#[test]
fn panels_tile_within_workspace_region() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let workspace_position = board.workspace(workspace_id).expect("workspace").position;

    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    let panel = board.panel(panel_id).expect("panel should exist");

    assert!((panel.layout.position[0] - (workspace_position[0] + WS_INNER_PAD)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[1] - (workspace_position[1] + WS_INNER_PAD)).abs() <= f32::EPSILON);
}

#[test]
fn workspaces_are_placed_apart() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    board
        .create_panel(editor_panel_options(), first_workspace)
        .expect("panel should spawn");
    let second_workspace = board.create_workspace("second");

    let first_position = board.workspace(first_workspace).expect("first workspace").position;
    let second_position = board.workspace(second_workspace).expect("second workspace").position;

    assert!(second_position[0] > first_position[0] + DEFAULT_PANEL_SIZE[0]);
}

#[test]
fn assign_panel_moves_it_to_target_workspace() {
    let mut board = Board::new();
    let source_workspace = board.create_workspace("source");
    let target_workspace = board.create_workspace("target");

    let panel_id = board
        .create_panel(editor_panel_options(), source_workspace)
        .expect("panel should spawn");

    let target_position = board.workspace(target_workspace).expect("target workspace").position;
    board.assign_panel_to_workspace(panel_id, target_workspace);

    let panel = board.panel(panel_id).expect("panel");
    assert_eq!(panel.workspace_id, target_workspace);
    assert!(panel.layout.position[0] >= target_position[0]);
}

#[test]
fn translating_workspace_moves_workspace_origin_and_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    let original_workspace_pos = board.workspace(workspace_id).expect("workspace").position;
    let original_panel_pos = board.panel(panel_id).expect("panel").layout.position;

    assert!(board.translate_workspace(workspace_id, [48.0, 24.0]));

    let workspace = board.workspace(workspace_id).expect("workspace");
    let panel = board.panel(panel_id).expect("panel");
    assert!((workspace.position[0] - (original_workspace_pos[0] + 48.0)).abs() <= f32::EPSILON);
    assert!((workspace.position[1] - (original_workspace_pos[1] + 24.0)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[0] - (original_panel_pos[0] + 48.0)).abs() <= f32::EPSILON);
    assert!((panel.layout.position[1] - (original_panel_pos[1] + 24.0)).abs() <= f32::EPSILON);
}

#[test]
fn translate_workspace_with_push_in_scope_ignores_out_of_scope_workspaces() {
    let mut board = Board::new();
    let alpha = board.create_workspace_at("alpha", [0.0, 40.0]);
    let beta = board.create_workspace_at("beta", [500.0, 40.0]);
    let gamma = board.create_workspace_at("gamma", [980.0, 40.0]);

    board
        .create_panel(editor_panel_options(), alpha)
        .expect("alpha panel should spawn");
    board
        .create_panel(editor_panel_options(), beta)
        .expect("beta panel should spawn");
    board
        .create_panel(editor_panel_options(), gamma)
        .expect("gamma panel should spawn");

    let beta_before = board.workspace(beta).expect("beta workspace").position;
    let gamma_before = board.workspace(gamma).expect("gamma workspace").position;

    assert!(board.translate_workspace_with_push_in_scope(alpha, [500.0, 0.0], &[alpha, beta]));

    let beta_after = board.workspace(beta).expect("beta workspace").position;
    let gamma_after = board.workspace(gamma).expect("gamma workspace").position;

    assert!(
        beta_after[0] > beta_before[0],
        "expected beta to move right from {beta_before:?}, got {beta_after:?}"
    );
    assert!(
        vec2_eq(gamma_after, gamma_before),
        "expected out-of-scope gamma workspace to stay at {gamma_before:?}, got {gamma_after:?}"
    );
}

#[test]
fn sync_workspace_metadata_updates_only_templated_workspaces() {
    let mut board = Board::new();
    let templated_workspace = board.create_workspace("stale");
    let manual_workspace = board.create_workspace("manual");

    {
        let workspace = board.workspace_mut(templated_workspace).expect("templated workspace");
        workspace.template = Some(WorkspaceTemplateRef {
            workspace_index: 0,
            workspace_name: "template".to_string(),
        });
        workspace.cwd = Some(PathBuf::from("/tmp/old"));
    }
    {
        let workspace = board.workspace_mut(manual_workspace).expect("manual workspace");
        workspace.cwd = Some(PathBuf::from("/tmp/manual"));
    }

    let config = Config {
        workspaces: vec![WorkspaceConfig {
            name: "synced".to_string(),
            color: None,
            cwd: Some("~/repo".to_string()),
            position: None,
            terminals: Vec::new(),
        }],
        ..Config::default()
    };

    board.sync_workspace_metadata(&config);

    let templated = board.workspace(templated_workspace).expect("templated workspace");
    assert_eq!(templated.name, "synced");
    assert_eq!(templated.cwd, Some(Config::expand_tilde("~/repo")));

    let manual = board.workspace(manual_workspace).expect("manual workspace");
    assert_eq!(manual.name, "manual");
    assert_eq!(manual.cwd, Some(PathBuf::from("/tmp/manual")));
}

#[test]
fn explicit_position_overrides_default_tiling() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("test");

    board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");

    let click_pos = [800.0, 600.0];
    let panel_id = board
        .create_panel(
            PanelOptions {
                position: Some(click_pos),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("second panel should spawn");

    let panel = board.panel(panel_id).expect("panel should exist");
    assert!(
        (panel.layout.position[0] - click_pos[0]).abs() <= f32::EPSILON
            && (panel.layout.position[1] - click_pos[1]).abs() <= f32::EPSILON,
        "panel should be placed at the explicit click position ({click_pos:?}), not at the default tile position ({:?})",
        panel.layout.position,
    );
}

#[test]
fn close_panels_in_workspace_keeps_workspace_available() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("alpha");
    let other_workspace_id = board.create_workspace("beta");
    let first = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("first panel should spawn");
    let second = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("second panel should spawn");
    let other_panel = board
        .create_panel(editor_panel_options(), other_workspace_id)
        .expect("other panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Rows);

    let closed = board.close_panels_in_workspace(workspace_id);
    board.remove_empty_workspaces();

    assert_eq!(closed, vec![first, second]);
    assert!(board.panel(first).is_none());
    assert!(board.panel(second).is_none());
    assert!(board.panel(other_panel).is_some());
    assert_eq!(board.focused, None);
    assert_eq!(board.active_workspace, Some(workspace_id));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").layout,
        Some(WorkspaceLayout::Rows)
    );
    assert!(board.workspace(workspace_id).expect("workspace").panels.is_empty());
}

#[test]
fn align_workspaces_horizontally_arranges_in_row() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    let second_workspace = board.create_workspace("second");
    let third_workspace = board.create_workspace("third");

    board.move_workspace(first_workspace, [100.0, 200.0]);
    board.move_workspace(second_workspace, [500.0, 50.0]);
    board.move_workspace(third_workspace, [300.0, 400.0]);

    board.align_workspaces_horizontally(&[first_workspace, second_workspace, third_workspace]);

    let first_position = board.workspace(first_workspace).expect("first").position;
    let third_position = board.workspace(third_workspace).expect("third").position;
    let second_position = board.workspace(second_workspace).expect("second").position;

    assert!((first_position[1] - third_position[1]).abs() <= f32::EPSILON);
    assert!((third_position[1] - second_position[1]).abs() <= f32::EPSILON);
    assert!(third_position[0] > first_position[0], "third should be right of first");
    assert!(
        second_position[0] > third_position[0],
        "second should be right of third"
    );
}

#[test]
fn align_workspaces_horizontally_only_moves_selected_workspaces() {
    let mut board = Board::new();
    let first_workspace = board.create_workspace("first");
    let second_workspace = board.create_workspace("second");
    let third_workspace = board.create_workspace("third");

    board.move_workspace(first_workspace, [100.0, 200.0]);
    board.move_workspace(second_workspace, [500.0, 50.0]);
    board.move_workspace(third_workspace, [20.0, 20.0]);

    let original_third_position = board.workspace(third_workspace).expect("third").position;
    let leftmost = board
        .align_workspaces_horizontally(&[first_workspace, second_workspace])
        .expect("aligned workspace");

    assert_eq!(leftmost, first_workspace);
    let current_third_position = board.workspace(third_workspace).expect("third").position;
    assert!(
        vec2_eq(current_third_position, original_third_position),
        "expected detached workspace position {original_third_position:?}, got {current_third_position:?}"
    );
}

#[test]
fn adding_panel_pushes_colliding_workspace() {
    let mut board = Board::new();
    let expanding_workspace = board.create_workspace("expanding");
    let colliding_workspace = board.create_workspace("colliding");

    board
        .create_panel(editor_panel_options(), expanding_workspace)
        .expect("first panel should spawn");
    board.arrange_workspace(expanding_workspace, WorkspaceLayout::Columns);

    board
        .create_panel(editor_panel_options(), colliding_workspace)
        .expect("colliding panel should spawn");
    assert!(board.move_workspace(colliding_workspace, [620.0, 40.0]));

    let initial_position = board
        .workspace(colliding_workspace)
        .expect("colliding workspace")
        .position;

    board
        .create_panel(editor_panel_options(), expanding_workspace)
        .expect("second panel should spawn");

    let moved_position = board
        .workspace(colliding_workspace)
        .expect("colliding workspace")
        .position;
    assert!(
        moved_position[0] > initial_position[0],
        "expected workspace to move right from {initial_position:?}, got {moved_position:?}"
    );
}

#[test]
fn assigning_panel_pushes_colliding_workspace() {
    let mut board = Board::new();
    let source_workspace = board.create_workspace("source");
    let target_workspace = board.create_workspace("target");
    let colliding_workspace = board.create_workspace("colliding");

    let moved_panel = board
        .create_panel(editor_panel_options(), source_workspace)
        .expect("source panel should spawn");
    board
        .create_panel(editor_panel_options(), target_workspace)
        .expect("target panel should spawn");
    board.arrange_workspace(target_workspace, WorkspaceLayout::Columns);

    board
        .create_panel(editor_panel_options(), colliding_workspace)
        .expect("colliding panel should spawn");
    assert!(board.move_workspace(colliding_workspace, [620.0, 40.0]));

    let initial_position = board
        .workspace(colliding_workspace)
        .expect("colliding workspace")
        .position;

    board.assign_panel_to_workspace(moved_panel, target_workspace);

    let moved_position = board
        .workspace(colliding_workspace)
        .expect("colliding workspace")
        .position;
    assert!(
        moved_position[0] > initial_position[0],
        "expected workspace to move right from {initial_position:?}, got {moved_position:?}"
    );
}

#[test]
fn restored_empty_workspaces_are_removed_during_cleanup() {
    let state = RuntimeState {
        active_workspace_local_id: Some("empty".to_string()),
        workspaces: vec![
            WorkspaceState {
                local_id: "empty".to_string(),
                name: "empty".to_string(),
                cwd: None,
                position: Some([0.0, 40.0]),
                template: None,
                layout: None,
                panels: Vec::new(),
            },
            WorkspaceState {
                local_id: "filled".to_string(),
                name: "filled".to_string(),
                cwd: None,
                position: Some([640.0, 40.0]),
                template: None,
                layout: None,
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "notes".to_string(),
                    kind: PanelKind::Editor,
                    command: None,
                    args: Vec::new(),
                    cwd: None,
                    ssh_connection: None,
                    rows: 24,
                    cols: 80,
                    resume: PanelResume::Fresh,
                    position: Some([640.0, 40.0]),
                    size: None,
                    session_binding: None,
                    template: None,
                    editor_content: None,
                }],
            },
        ],
        ..RuntimeState::default()
    };

    let mut board = Board::from_runtime_state(&state).expect("board");

    board.remove_empty_workspaces();

    assert_eq!(board.workspaces.len(), 1);
    assert_eq!(board.workspaces[0].local_id, "filled");
    assert_eq!(board.active_workspace, Some(board.workspaces[0].id));
}

#[test]
fn persisted_ssh_panels_restore_as_disconnected_snapshots() {
    let transcript_root = tempfile::tempdir().expect("tempdir");
    std::fs::write(transcript_root.path().join("ssh-panel.bin"), b"restored ssh prompt\r\n").expect("write transcript");
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "remote".to_string(),
            name: "Remote".to_string(),
            cwd: None,
            position: Some([0.0, 40.0]),
            template: None,
            layout: None,
            panels: vec![PanelState {
                local_id: "ssh-panel".to_string(),
                name: "prod".to_string(),
                kind: PanelKind::Ssh,
                command: None,
                args: Vec::new(),
                cwd: None,
                ssh_connection: Some(SshConnection {
                    host: "prod".to_string(),
                    user: Some("deploy".to_string()),
                    ..SshConnection::default()
                }),
                rows: 24,
                cols: 80,
                resume: PanelResume::Fresh,
                position: Some([0.0, 40.0]),
                size: None,
                session_binding: None,
                template: None,
                editor_content: None,
            }],
        }],
        ..RuntimeState::default()
    };

    let board = Board::from_runtime_state_with_transcripts(&state, Some(transcript_root.path())).expect("board");
    let panel = board.panels.first().expect("panel");

    assert_eq!(panel.ssh_status(), Some(SshConnectionStatus::Disconnected));
    assert_eq!(
        panel.terminal().expect("terminal").last_lines_text(1),
        "restored ssh prompt"
    );
}

#[test]
fn resize_panel_pushes_sibling() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("test");

    let panel_a = board
        .create_panel(
            PanelOptions {
                position: Some([0.0, 0.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel A should spawn");
    let panel_b = board
        .create_panel(
            PanelOptions {
                position: Some([220.0, 0.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel B should spawn");

    board.resize_panel(panel_a, [300.0, 200.0]);

    let panel_b_position = board.panel(panel_b).expect("panel B").layout.position;
    assert!(
        panel_b_position[0] >= 300.0 + TILE_GAP - 1.0,
        "panel B should be pushed right, got x={}",
        panel_b_position[0],
    );
}

#[test]
fn resize_panel_with_workspace_scope_ignores_out_of_scope_workspaces() {
    let mut board = Board::new();
    let alpha = board.create_workspace_at("alpha", [0.0, 40.0]);
    let beta = board.create_workspace_at("beta", [500.0, 40.0]);
    let gamma = board.create_workspace_at("gamma", [980.0, 40.0]);

    board
        .create_panel(
            PanelOptions {
                position: Some([20.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            alpha,
        )
        .expect("alpha panel should spawn");
    board
        .create_panel(
            PanelOptions {
                position: Some([520.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            beta,
        )
        .expect("beta panel should spawn");
    let gamma_panel = board
        .create_panel(
            PanelOptions {
                position: Some([1000.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            gamma,
        )
        .expect("gamma panel should spawn");

    let alpha_panel = board.workspace(alpha).expect("alpha workspace").panels[0];
    let beta_before = board.workspace(beta).expect("beta workspace").position;
    let gamma_before = board.workspace(gamma).expect("gamma workspace").position;
    let gamma_panel_before = board.panel(gamma_panel).expect("gamma panel").layout.position;

    assert!(board.resize_panel_with_workspace_scope(alpha_panel, [650.0, 360.0], &[alpha, beta]));

    let beta_after = board.workspace(beta).expect("beta workspace").position;
    let gamma_after = board.workspace(gamma).expect("gamma workspace").position;
    let gamma_panel_after = board.panel(gamma_panel).expect("gamma panel").layout.position;

    assert!(
        beta_after[0] > beta_before[0],
        "expected beta to move right from {beta_before:?}, got {beta_after:?}"
    );
    assert!(
        vec2_eq(gamma_after, gamma_before),
        "expected out-of-scope gamma workspace to stay at {gamma_before:?}, got {gamma_after:?}"
    );
    assert!(
        vec2_eq(gamma_panel_after, gamma_panel_before),
        "expected out-of-scope gamma panel to stay at {gamma_panel_before:?}, got {gamma_panel_after:?}"
    );
}

#[test]
fn resize_panel_pushes_sibling_vertically_when_height_growth_dominates() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("test");

    let panel_a = board
        .create_panel(
            PanelOptions {
                position: Some([0.0, 0.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel A should spawn");
    let panel_b = board
        .create_panel(
            PanelOptions {
                position: Some([0.0, 220.0]),
                size: Some([200.0, 200.0]),
                ..editor_panel_options()
            },
            workspace_id,
        )
        .expect("panel B should spawn");

    let panel_b_before = board.panel(panel_b).expect("panel B").layout.position;

    board.resize_panel(panel_a, [240.0, 340.0]);

    let panel_b_after = board.panel(panel_b).expect("panel B").layout.position;
    assert!(
        panel_b_after[1] >= 340.0 + TILE_GAP - 1.0,
        "panel B should be pushed down, got {panel_b_after:?}"
    );
    assert!(
        (panel_b_after[0] - panel_b_before[0]).abs() <= f32::EPSILON,
        "panel B x should stay at {}, got {}",
        panel_b_before[0],
        panel_b_after[0],
    );
}

#[test]
fn resize_panel_pushes_neighbor_workspaces_horizontally_when_width_growth_dominates() {
    let mut board = Board::new();
    let alpha = board.create_workspace_at("alpha", [0.0, 40.0]);
    let beta = board.create_workspace_at("beta", [500.0, 40.0]);
    let gamma = board.create_workspace_at("gamma", [980.0, 40.0]);

    let alpha_panel = board
        .create_panel(
            PanelOptions {
                position: Some([20.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            alpha,
        )
        .expect("alpha panel should spawn");
    board
        .create_panel(
            PanelOptions {
                position: Some([520.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            beta,
        )
        .expect("beta panel should spawn");
    board
        .create_panel(
            PanelOptions {
                position: Some([1000.0, 60.0]),
                size: Some([420.0, 300.0]),
                ..editor_panel_options()
            },
            gamma,
        )
        .expect("gamma panel should spawn");

    let beta_before = board.workspace(beta).expect("beta workspace").position;
    let gamma_before = board.workspace(gamma).expect("gamma workspace").position;

    board.resize_panel(alpha_panel, [650.0, 360.0]);

    let beta_after = board.workspace(beta).expect("beta workspace").position;
    let gamma_after = board.workspace(gamma).expect("gamma workspace").position;
    assert!(
        beta_after[0] > beta_before[0],
        "expected beta to move right from {beta_before:?}, got {beta_after:?}"
    );
    assert!(
        gamma_after[0] > gamma_before[0],
        "expected gamma to move right from {gamma_before:?}, got {gamma_after:?}"
    );
    assert!(
        (beta_after[1] - beta_before[1]).abs() <= f32::EPSILON,
        "expected beta y to stay at {}, got {}",
        beta_before[1],
        beta_after[1],
    );
    assert!(
        (gamma_after[1] - gamma_before[1]).abs() <= f32::EPSILON,
        "expected gamma y to stay at {}, got {}",
        gamma_before[1],
        gamma_after[1],
    );
}
