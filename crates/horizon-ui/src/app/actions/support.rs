use std::collections::{BTreeMap, HashSet};

use egui::{Button, Color32, Ui};
use horizon_core::{PresetConfig, WorkspaceId};

use crate::command_palette::{PanelEntry, PresetEntry, WorkspaceEntry};
use crate::theme;

use super::PresetPickerAction;
use crate::app::DetachedWorkspaceViewportState;

pub(super) fn preset_picker_heading(target_workspace: Option<WorkspaceId>) -> &'static str {
    if target_workspace.is_some() {
        "New Terminal"
    } else {
        "New Workspace"
    }
}

pub(super) fn render_preset_picker_row(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    let label = if let Some(alias) = &preset.alias {
        format!("{} ({})", preset.name, alias)
    } else {
        preset.name.clone()
    };

    match target_workspace {
        Some(workspace_id) => render_panel_preset_picker_row(ui, workspace_id, canvas_pos, preset, label),
        None => render_workspace_preset_picker_row(ui, canvas_pos, preset, label),
    }
}

fn render_panel_preset_picker_row(
    ui: &mut Ui,
    workspace_id: WorkspaceId,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
    label: String,
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    ui.horizontal(|ui| {
        let create_text = egui::RichText::new(label).size(12.5).color(theme::FG_SOFT);
        if ui.add(Button::new(create_text).frame(false)).clicked() {
            selected_action = Some(PresetPickerAction::CreatePanel {
                workspace_id,
                preset: preset.clone(),
                canvas_pos: Some(canvas_pos),
            });
        }

        let dir_text = egui::RichText::new("Dir").size(11.0).color(theme::FG_DIM);
        if ui.add(Button::new(dir_text).frame(false)).clicked() {
            selected_action = Some(PresetPickerAction::ChooseDirectory {
                workspace_id,
                preset: preset.clone(),
                canvas_pos: Some(canvas_pos),
            });
        }
    });
    selected_action
}

fn render_workspace_preset_picker_row(
    ui: &mut Ui,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
    label: String,
) -> Option<PresetPickerAction> {
    let create_text = egui::RichText::new(label).size(12.5).color(theme::FG_SOFT);
    if !ui.add(Button::new(create_text).frame(false)).clicked() {
        return None;
    }

    Some(if preset.requires_workspace_cwd() {
        PresetPickerAction::CreateWorkspace {
            canvas_pos,
            preset: preset.clone(),
        }
    } else {
        PresetPickerAction::CreateWorkspaceDirect {
            canvas_pos,
            preset: preset.clone(),
        }
    })
}

pub(super) fn detached_workspace_ids(
    board: &horizon_core::Board,
    detached_workspaces: &BTreeMap<String, DetachedWorkspaceViewportState>,
) -> HashSet<WorkspaceId> {
    detached_workspaces
        .keys()
        .filter_map(|local_id| board.workspace_id_by_local_id(local_id))
        .collect()
}

pub(super) fn command_palette_workspace_entries(
    board: &horizon_core::Board,
    detached_workspace_ids: &HashSet<WorkspaceId>,
    active_workspace: Option<WorkspaceId>,
) -> Vec<WorkspaceEntry> {
    board
        .workspaces
        .iter()
        .filter(|workspace| !detached_workspace_ids.contains(&workspace.id))
        .map(|workspace| {
            let (r, g, b) = workspace.accent();
            WorkspaceEntry {
                id: workspace.id,
                name: workspace.name.clone(),
                color: Color32::from_rgb(r, g, b),
                panel_count: workspace.panels.len(),
                is_active: active_workspace == Some(workspace.id),
            }
        })
        .collect()
}

pub(super) fn command_palette_panel_entries(
    board: &horizon_core::Board,
    detached_workspace_ids: &HashSet<WorkspaceId>,
) -> Vec<PanelEntry> {
    board
        .panels
        .iter()
        .filter(|panel| !detached_workspace_ids.contains(&panel.workspace_id))
        .map(|panel| {
            let workspace_name = board
                .workspace(panel.workspace_id)
                .map_or_else(String::new, |workspace| workspace.name.clone());
            PanelEntry {
                id: panel.id,
                title: panel.display_title().into_owned(),
                workspace_name,
                cwd: panel.launch_cwd.as_ref().map(|path| path.display().to_string()),
            }
        })
        .collect()
}

pub(super) fn command_palette_preset_entries(presets: &[PresetConfig]) -> Vec<PresetEntry> {
    presets
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            let mut keywords = vec![preset.kind.display_name().to_ascii_lowercase()];
            if let Some(alias) = &preset.alias {
                keywords.push(alias.clone());
            }
            if let Some(connection) = &preset.ssh_connection {
                keywords.push(connection.host.clone());
                if let Some(user) = &connection.user {
                    keywords.push(user.clone());
                }
            }

            let detail = if let Some(connection) = &preset.ssh_connection {
                connection.display_label()
            } else if let Some(alias) = &preset.alias {
                format!("{}  {}", preset.kind.display_name(), alias)
            } else {
                preset.kind.display_name().to_string()
            };

            PresetEntry {
                index,
                label: preset.name.clone(),
                detail,
                keywords,
            }
        })
        .collect()
}
