mod command_palette;
mod interaction;
mod layout;
mod panels;
mod pickers;
mod search;
mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;

use horizon_core::{PanelOptions, PresetConfig, WorkspaceId};

use self::support::detached_workspace_ids;

fn workspace_cwd(board: &horizon_core::Board, workspace_id: WorkspaceId) -> Option<PathBuf> {
    board
        .workspace(workspace_id)
        .and_then(|workspace| workspace.cwd.clone())
}

enum PresetPickerAction {
    CreatePanel {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
    ChooseDirectory {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
    CreateWorkspace {
        canvas_pos: [f32; 2],
        preset: PresetConfig,
    },
    CreateWorkspaceDirect {
        canvas_pos: [f32; 2],
        preset: PresetConfig,
    },
}

fn inherit_workspace_cwd(options: &mut PanelOptions, workspace_cwd: Option<&PathBuf>) {
    if options.cwd.is_none()
        && let Some(workspace_cwd) = workspace_cwd
    {
        options.cwd = Some(workspace_cwd.clone());
    }
}

fn update_workspace_cwd(workspace: Option<&mut horizon_core::Workspace>, path: Option<&PathBuf>) {
    if let Some(path) = path
        && let Some(workspace) = workspace
    {
        workspace.cwd = Some(path.clone());
    }
}

fn align_attached_workspaces(
    board: &mut horizon_core::Board,
    detached_workspaces: &BTreeMap<String, horizon_core::WindowConfig>,
) -> Option<WorkspaceId> {
    let detached_workspace_ids = detached_workspace_ids(board, detached_workspaces);
    let workspace_ids: Vec<_> = board
        .workspaces
        .iter()
        .filter(|workspace| !detached_workspace_ids.contains(&workspace.id))
        .map(|workspace| workspace.id)
        .collect();
    board.align_workspaces_horizontally(&workspace_ids)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::path::PathBuf;

    use egui::{Color32, Pos2, Rect};
    use horizon_core::{Board, PanelId, PanelKind, PanelOptions, PresetConfig, WindowConfig, Workspace, WorkspaceId};

    use super::layout::{
        canvas_rect_for_layout, estimated_settings_bar_rect, estimated_settings_panel_rect,
        panel_focus_target_at_pointer_press,
    };
    use super::support::{
        command_palette_panel_entries, command_palette_preset_entries, command_palette_workspace_entries,
        detached_workspace_ids,
    };
    use super::{align_attached_workspaces, inherit_workspace_cwd, update_workspace_cwd, workspace_cwd};
    use crate::app::settings::SETTINGS_BAR_HEIGHT;
    use crate::app::{SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

    #[test]
    fn inherit_workspace_cwd_populates_missing_panel_cwd() {
        let mut options = PanelOptions::default();
        let workspace_path = PathBuf::from("/repo");

        inherit_workspace_cwd(&mut options, Some(&workspace_path));

        assert_eq!(options.cwd, Some(workspace_path));
    }

    #[test]
    fn inherit_workspace_cwd_preserves_explicit_panel_cwd() {
        let panel_path = PathBuf::from("/panel");
        let workspace_path = PathBuf::from("/repo");
        let mut options = PanelOptions {
            cwd: Some(panel_path.clone()),
            ..PanelOptions::default()
        };

        inherit_workspace_cwd(&mut options, Some(&workspace_path));

        assert_eq!(options.cwd, Some(panel_path));
    }

    #[test]
    fn update_workspace_cwd_promotes_selected_panel_directory() {
        let mut workspace = Workspace::new(WorkspaceId(1), "alpha".to_string(), 0);
        let selected_path = PathBuf::from("/repo");

        update_workspace_cwd(Some(&mut workspace), Some(&selected_path));

        assert_eq!(workspace.cwd, Some(selected_path));
    }

    #[test]
    fn update_workspace_cwd_keeps_existing_directory_when_picker_is_skipped() {
        let existing_path = PathBuf::from("/repo");
        let mut workspace = Workspace::new(WorkspaceId(1), "alpha".to_string(), 0);
        workspace.cwd = Some(existing_path.clone());

        update_workspace_cwd(Some(&mut workspace), None);

        assert_eq!(workspace.cwd, Some(existing_path));
    }

    #[test]
    fn workspace_cwd_reads_workspace_default_directory() {
        let mut board = Board::new();
        let workspace_id = board.create_workspace("alpha");
        let path = PathBuf::from("/repo");
        board.workspace_mut(workspace_id).expect("workspace").cwd = Some(path.clone());

        assert_eq!(workspace_cwd(&board, workspace_id), Some(path));
    }

    #[test]
    fn command_palette_preset_entries_include_ssh_keywords() {
        let presets = vec![PresetConfig {
            name: "SSH: prod-api".to_string(),
            alias: Some("pa".to_string()),
            kind: PanelKind::Ssh,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: Some(horizon_core::SshConnection {
                host: "prod-api".to_string(),
                user: Some("deploy".to_string()),
                ..horizon_core::SshConnection::default()
            }),
        }];

        let entries = command_palette_preset_entries(&presets);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail, "deploy@prod-api");
        assert!(entries[0].keywords.iter().any(|keyword| keyword == "deploy"));
    }

    #[test]
    fn estimated_settings_panel_rect_uses_default_wide_fallback() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));

        let rect = estimated_settings_panel_rect(viewport, true, None).expect("settings rect");

        assert_eq!(rect.min, Pos2::new(840.0, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(1200.0, 752.0));
    }

    #[test]
    fn estimated_settings_panel_rect_clamps_narrow_fallback_width() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(700.0, 800.0));

        let rect = estimated_settings_panel_rect(viewport, true, None).expect("settings rect");

        assert_eq!(rect.min, Pos2::new(360.0, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(700.0, 752.0));
    }

    #[test]
    fn estimated_settings_panel_rect_prefers_remembered_panel_state() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));
        let remembered = Rect::from_min_max(Pos2::new(900.0, 60.0), Pos2::new(1200.0, 720.0));

        let rect = estimated_settings_panel_rect(viewport, true, Some(remembered)).expect("settings rect");

        assert_eq!(rect, remembered);
    }

    #[test]
    fn estimated_settings_rects_close_when_settings_are_hidden() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));

        assert_eq!(estimated_settings_panel_rect(viewport, false, None), None);
        assert_eq!(estimated_settings_bar_rect(viewport, false, None), None);
    }

    #[test]
    fn canvas_rect_for_layout_excludes_sidebar_settings_panel_and_bar() {
        let viewport = Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0));
        let settings_panel = Rect::from_min_max(Pos2::new(840.0, TOOLBAR_HEIGHT), Pos2::new(1200.0, 752.0));
        let settings_bar = Rect::from_min_max(Pos2::new(0.0, 800.0 - SETTINGS_BAR_HEIGHT), Pos2::new(1200.0, 800.0));

        let rect = canvas_rect_for_layout(viewport, true, Some(settings_panel), Some(settings_bar));

        assert_eq!(rect.min, Pos2::new(SIDEBAR_WIDTH, TOOLBAR_HEIGHT));
        assert_eq!(rect.max, Pos2::new(840.0, 800.0 - SETTINGS_BAR_HEIGHT));
    }

    #[test]
    fn detached_workspace_ids_resolve_from_local_ids() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");
        let detached_local_id = board.workspace(detached).expect("detached workspace").local_id.clone();

        let mut detached_workspaces = BTreeMap::new();
        detached_workspaces.insert(detached_local_id, WindowConfig::default());

        let ids = detached_workspace_ids(&board, &detached_workspaces);

        assert!(ids.contains(&detached));
        assert!(!ids.contains(&attached));
    }

    #[test]
    fn command_palette_workspace_entries_skip_detached_workspaces() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");

        let entries = command_palette_workspace_entries(&board, &HashSet::from([detached]), Some(attached));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, attached);
        assert_eq!(entries[0].name, "attached");
        assert_eq!(entries[0].color, Color32::from_rgb(137, 180, 250));
        assert!(entries[0].is_active);
    }

    #[test]
    fn command_palette_panel_entries_skip_panels_in_detached_workspaces() {
        let mut board = Board::new();
        let attached = board.create_workspace("attached");
        let detached = board.create_workspace("detached");
        let attached_panel = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Editor,
                    command: Some("attached.md".to_string()),
                    ..PanelOptions::default()
                },
                attached,
            )
            .expect("attached panel");
        board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Editor,
                    command: Some("detached.md".to_string()),
                    ..PanelOptions::default()
                },
                detached,
            )
            .expect("detached panel");

        let entries = command_palette_panel_entries(&board, &HashSet::from([detached]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, attached_panel);
        assert_eq!(entries[0].workspace_name, "attached");
    }

    #[test]
    fn align_attached_workspaces_ignores_detached_workspaces() {
        let mut board = Board::new();
        let left = board.create_workspace("left");
        let detached = board.create_workspace("detached");
        let right = board.create_workspace("right");
        board.move_workspace(left, [100.0, 200.0]);
        board.move_workspace(detached, [300.0, 50.0]);
        board.move_workspace(right, [500.0, 400.0]);

        let detached_local_id = board.workspace(detached).expect("detached workspace").local_id.clone();
        let detached_position = board.workspace(detached).expect("detached workspace").position;
        let detached_workspaces = BTreeMap::from([(detached_local_id, WindowConfig::default())]);

        let leftmost = align_attached_workspaces(&mut board, &detached_workspaces);

        assert_eq!(leftmost, Some(left));
        assert!(board.workspace(detached).is_some_and(|workspace| {
            workspace
                .position
                .iter()
                .zip(detached_position)
                .all(|(current, original)| (current - original).abs() <= f32::EPSILON)
        }));
    }

    #[test]
    fn panel_focus_target_prefers_existing_focused_panel_when_rects_overlap() {
        let panel_a = PanelId(1);
        let panel_b = PanelId(2);
        let panel_rects = HashMap::from([
            (
                panel_a,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
            (
                panel_b,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
        ]);

        let target = panel_focus_target_at_pointer_press(
            &[panel_a, panel_b],
            &panel_rects,
            Some(panel_a),
            Pos2::new(40.0, 40.0),
        );

        assert_eq!(target, Some(panel_a));
    }

    #[test]
    fn panel_focus_target_uses_frontmost_panel_order_for_unfocused_overlap() {
        let panel_a = PanelId(1);
        let panel_b = PanelId(2);
        let panel_rects = HashMap::from([
            (
                panel_a,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
            (
                panel_b,
                Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(80.0, 80.0)),
            ),
        ]);

        let target =
            panel_focus_target_at_pointer_press(&[panel_a, panel_b], &panel_rects, None, Pos2::new(40.0, 40.0));

        assert_eq!(target, Some(panel_b));
    }
}
