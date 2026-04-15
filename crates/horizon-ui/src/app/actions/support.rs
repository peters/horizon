use std::collections::{BTreeMap, HashSet};

use egui::text::{LayoutJob, TextFormat};
use egui::{Button, Color32, FontId, Ui};
use horizon_core::{PanelKind, PresetConfig, WorkspaceId};

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

pub(super) fn render_grouped_preset_rows(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    presets: &[PresetConfig],
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    let mut any_group_rendered = false;

    for &category in &CATEGORY_ORDER {
        let mut group_started = false;

        for preset in presets {
            if preset_category(preset) != category {
                continue;
            }

            if !group_started {
                if any_group_rendered {
                    ui.add_space(2.0);
                    ui.separator();
                    ui.add_space(2.0);
                }
                if category != PresetCategory::Shell {
                    ui.label(egui::RichText::new(category.label()).size(10.0).color(theme::FG_DIM()));
                    ui.add_space(1.0);
                }
                group_started = true;
            }

            if let Some(action) = render_preset_picker_row(ui, target_workspace, canvas_pos, preset) {
                selected_action = Some(action);
            }
        }

        if group_started {
            any_group_rendered = true;
        }
    }

    selected_action
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PresetCategory {
    Shell,
    Agent,
    Tool,
    Remote,
}

const CATEGORY_ORDER: [PresetCategory; 4] = [
    PresetCategory::Shell,
    PresetCategory::Agent,
    PresetCategory::Tool,
    PresetCategory::Remote,
];

impl PresetCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Agent => "Agents",
            Self::Tool => "Tools",
            Self::Remote => "Remote",
        }
    }
}

fn preset_ssh_connection(preset: &PresetConfig) -> Option<&horizon_core::SshConnection> {
    if preset.kind == PanelKind::Ssh {
        preset.ssh_connection.as_ref()
    } else {
        None
    }
}

fn preset_category(preset: &PresetConfig) -> PresetCategory {
    if preset.kind == PanelKind::Ssh {
        PresetCategory::Remote
    } else if preset.kind.is_agent() {
        PresetCategory::Agent
    } else if matches!(preset.kind, PanelKind::Shell) {
        PresetCategory::Shell
    } else {
        PresetCategory::Tool
    }
}

fn preset_button_label(preset: &PresetConfig) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &preset.name,
        0.0,
        TextFormat {
            font_id: FontId::proportional(12.5),
            color: theme::FG_SOFT(),
            ..Default::default()
        },
    );
    if let Some(alias) = &preset.alias {
        job.append(
            &format!("  {alias}"),
            0.0,
            TextFormat {
                font_id: FontId::monospace(10.0),
                color: theme::FG_DIM(),
                ..Default::default()
            },
        );
    }
    job
}

fn render_preset_picker_row(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    match target_workspace {
        Some(workspace_id) => render_panel_preset_picker_row(ui, workspace_id, canvas_pos, preset),
        None => render_workspace_preset_picker_row(ui, canvas_pos, preset),
    }
}

fn render_panel_preset_picker_row(
    ui: &mut Ui,
    workspace_id: WorkspaceId,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    ui.horizontal(|ui| {
        if ui.add(Button::new(preset_button_label(preset)).frame(false)).clicked() {
            selected_action = Some(PresetPickerAction::CreatePanel {
                workspace_id,
                preset: preset.clone(),
                canvas_pos: Some(canvas_pos),
            });
        }

        let dir_text = egui::RichText::new("Dir").size(11.0).color(theme::FG_DIM());
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
) -> Option<PresetPickerAction> {
    if !ui.add(Button::new(preset_button_label(preset)).frame(false)).clicked() {
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
            let ssh_connection = preset_ssh_connection(preset);
            let mut keywords = vec![preset.kind.display_name().to_ascii_lowercase()];
            if let Some(alias) = &preset.alias {
                keywords.push(alias.clone());
            }
            if let Some(connection) = ssh_connection {
                keywords.push(connection.host.clone());
                if let Some(user) = &connection.user {
                    keywords.push(user.clone());
                }
            }

            let detail = if let Some(connection) = ssh_connection {
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

#[cfg(test)]
mod tests {
    use horizon_core::{PanelKind, PanelResume, PresetConfig, SshConnection};

    use super::{PresetCategory, command_palette_preset_entries, preset_category};

    fn shell_preset_with_stale_ssh_metadata() -> PresetConfig {
        PresetConfig {
            name: "Shell".to_string(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: Some(SshConnection {
                host: "prod-api".to_string(),
                user: Some("deploy".to_string()),
                ..SshConnection::default()
            }),
        }
    }

    #[test]
    fn preset_category_ignores_stale_ssh_metadata_for_non_ssh_presets() {
        assert!(matches!(
            preset_category(&shell_preset_with_stale_ssh_metadata()),
            PresetCategory::Shell
        ));
    }

    #[test]
    fn command_palette_preset_entries_ignore_stale_ssh_metadata_for_non_ssh_presets() {
        let entries = command_palette_preset_entries(&[shell_preset_with_stale_ssh_metadata()]);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail, "Shell");
        assert!(!entries[0].keywords.iter().any(|keyword| keyword == "prod-api"));
        assert!(!entries[0].keywords.iter().any(|keyword| keyword == "deploy"));
    }
}
