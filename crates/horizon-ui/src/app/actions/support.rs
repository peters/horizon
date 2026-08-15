use std::collections::{BTreeMap, HashSet};

use egui::text::{LayoutJob, TextFormat};
use egui::{Button, Color32, FontId, Response, Ui, WidgetText};
use horizon_core::{PanelId, PanelKind, PresetConfig, WorkspaceId, flatten_line_separators};

use crate::command_palette::{PanelEntry, PresetEntry, WorkspaceEntry};
use crate::text::{BoundedSingleLineJob, painter_text_galley, stable_wrapped_hover_text_lazy};
use crate::theme;

use super::PresetPickerAction;
use crate::app::DetachedWorkspaceViewportState;

const PRESET_LABEL_RESERVATION_TOLERANCE: f32 = 0.5;
const PRESET_ALIAS_MAX_WIDTH_FRACTION: f32 = 0.4;

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
    row_width: f32,
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

            if let Some(action) = render_preset_picker_row(ui, target_workspace, canvas_pos, preset, row_width) {
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

fn preset_button_layout_job(ui: &Ui, preset: &PresetConfig, max_width: f32) -> LayoutJob {
    let name_font = FontId::proportional(12.5);
    let name_color = theme::FG_SOFT();
    let alias_font = FontId::monospace(10.0);
    let alias_color = theme::FG_DIM();
    let Some(alias) = &preset.alias else {
        let mut job = BoundedSingleLineJob::new(ui.ctx(), max_width);
        let _ = job.append(
            &preset.name,
            0.0,
            TextFormat {
                font_id: name_font,
                color: name_color,
                ..Default::default()
            },
        );
        return job.finish();
    };

    let alias_text = format!("  {alias}");
    let alias_budget = (max_width.max(0.0) * PRESET_ALIAS_MAX_WIDTH_FRACTION).max(0.0);
    let alias_galley = painter_text_galley(ui.painter(), &alias_text, &alias_font, alias_color, alias_budget);
    let display_alias = alias_galley.rows.first().map_or_else(String::new, |row| row.text());
    let alias_width = alias_galley.size().x;
    let name_width = (max_width - alias_width - PRESET_LABEL_RESERVATION_TOLERANCE).max(0.0);
    let display_name = if name_width > 0.0 {
        painter_text_galley(ui.painter(), &preset.name, &name_font, name_color, name_width)
            .rows
            .first()
            .map_or_else(String::new, |row| row.text())
    } else {
        String::new()
    };

    let mut job = BoundedSingleLineJob::new(ui.ctx(), max_width);
    if job.append(
        &display_name,
        0.0,
        TextFormat {
            font_id: name_font,
            color: name_color,
            ..Default::default()
        },
    ) {
        let _ = job.append(
            &display_alias,
            0.0,
            TextFormat {
                font_id: alias_font,
                color: alias_color,
                ..Default::default()
            },
        );
    }
    job.finish()
}

fn preset_button_label(ui: &Ui, preset: &PresetConfig, max_width: f32) -> WidgetText {
    WidgetText::Galley(ui.painter().layout_job(preset_button_layout_job(ui, preset, max_width)))
}

fn preset_button(ui: &mut Ui, preset: &PresetConfig, max_width: f32) -> Response {
    let label = preset_button_label(ui, preset, max_width);
    let hover_text = preset_hover_text(preset);
    let elided = preset_button_needs_tooltip(&label, &hover_text);
    let response = ui.add(Button::new(label).frame(false));
    if elided {
        stable_wrapped_hover_text_lazy(response, || hover_text)
    } else {
        response
    }
}

fn preset_button_needs_tooltip(label: &WidgetText, full_text: &str) -> bool {
    matches!(
        label,
        WidgetText::Galley(galley) if galley.elided || galley.job.text != full_text
    )
}

fn preset_hover_text(preset: &PresetConfig) -> String {
    let name = flatten_line_separators(&preset.name);
    match &preset.alias {
        Some(alias) => format!("{}  {}", name.as_ref(), flatten_line_separators(alias)),
        None => name.into_owned(),
    }
}

fn render_preset_picker_row(
    ui: &mut Ui,
    target_workspace: Option<WorkspaceId>,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
    row_width: f32,
) -> Option<PresetPickerAction> {
    match target_workspace {
        Some(workspace_id) => render_panel_preset_picker_row(ui, workspace_id, canvas_pos, preset, row_width),
        None => render_workspace_preset_picker_row(ui, canvas_pos, preset, row_width),
    }
}

fn render_panel_preset_picker_row(
    ui: &mut Ui,
    workspace_id: WorkspaceId,
    canvas_pos: [f32; 2],
    preset: &PresetConfig,
    row_width: f32,
) -> Option<PresetPickerAction> {
    let mut selected_action = None;
    ui.horizontal(|ui| {
        let label_width = (row_width - 44.0).max(0.0);
        if preset_button(ui, preset, label_width).clicked() {
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
    row_width: f32,
) -> Option<PresetPickerAction> {
    if !preset_button(ui, preset, row_width).clicked() {
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

// A detached workspace paints its own panels inside its own viewport, so a
// panel from one can never be fullscreened in the root window: both passes run
// in the same frame and would reflow the one PTY to two different grid sizes.
pub(super) fn fullscreen_panel_is_renderable(
    board: &horizon_core::Board,
    detached_workspaces: &BTreeMap<String, DetachedWorkspaceViewportState>,
    panel_id: PanelId,
) -> bool {
    board.panel(panel_id).is_some_and(|panel| {
        board
            .workspace(panel.workspace_id)
            .is_some_and(|workspace| !detached_workspaces.contains_key(&workspace.local_id))
    })
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

    use super::{
        PresetCategory, command_palette_preset_entries, preset_button_label, preset_button_layout_job,
        preset_button_needs_tooltip, preset_category, preset_hover_text, render_grouped_preset_rows,
    };

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

    #[test]
    fn preset_hover_text_preserves_full_flattened_name_and_alias() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "Deploy first\r\nsecond".to_string();
        preset.alias = Some("alias\u{2028}detail".to_string());

        assert_eq!(preset_hover_text(&preset), "Deploy first second  alias detail");
    }

    #[test]
    fn preset_button_label_is_single_line_and_width_bounded() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "deploy\nstaging with a deliberately long name".to_string();
        preset.alias = Some("alias\u{000B}next".to_string());

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut job = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                job = Some(preset_button_layout_job(ui, &preset, 80.0));
            });
        });
        let Some(job) = job else {
            panic!("preset job was not built");
        };

        assert!(job.text.contains('…'));
        assert!(job.text.contains("al"), "alias was erased from {:?}", job.text);
        assert!(job.text.ends_with('…'));
        assert!(!job.break_on_newline);
        assert_eq!(job.wrap.max_rows, 1);
        assert_eq!(job.wrap.overflow_character, Some('…'));
        assert!((job.wrap.max_width - 80.0).abs() < f32::EPSILON);
    }

    #[test]
    fn preset_button_widget_reuses_the_bounded_galley() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "deploy staging with a deliberately long name".to_string();
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut measured = None;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let egui::WidgetText::Galley(galley) = preset_button_label(ui, &preset, 80.0) else {
                    panic!("preset label did not retain its precomputed galley");
                };
                measured = Some((galley.size().x, galley.elided));
            });
        });

        let Some((width, elided)) = measured else {
            panic!("preset galley was not measured");
        };
        assert!(width <= 80.0);
        assert!(elided);
    }

    #[test]
    fn long_preset_name_keeps_the_alias_visible() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "Claude Code - production deploy runner".to_string();
        preset.alias = Some("ccp".to_string());
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut rendered = String::new();
        let mut needs_tooltip = false;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let label = preset_button_label(ui, &preset, 160.0);
                needs_tooltip = preset_button_needs_tooltip(&label, &preset_hover_text(&preset));
                let egui::WidgetText::Galley(galley) = label else {
                    panic!("preset label did not retain its precomputed galley");
                };
                rendered = galley.rows.first().map_or_else(String::new, |row| row.text());
            });
        });

        assert!(rendered.contains('…'));
        assert!(rendered.ends_with("  ccp"), "alias was elided from {rendered:?}");
        assert!(needs_tooltip);
    }

    #[test]
    fn pathological_preset_sections_share_one_shaping_budget() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "n".repeat(10_000);
        preset.alias = Some("a".repeat(10_000));
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut job = None;

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                job = Some(preset_button_layout_job(ui, &preset, 320.0));
            });
        });
        let Some(job) = job else {
            panic!("preset job was not built");
        };

        assert!(job.text.chars().count() <= 513);
    }

    #[test]
    fn long_alias_keeps_the_preset_name_visible() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "Production deploy".to_string();
        preset.alias = Some("alias-with-deliberately-excessive-detail".to_string());
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut rendered = String::new();

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let egui::WidgetText::Galley(galley) = preset_button_label(ui, &preset, 160.0) else {
                    panic!("preset label did not retain its precomputed galley");
                };
                rendered = galley.rows.first().map_or_else(String::new, |row| row.text());
            });
        });

        assert!(rendered.starts_with("Production"), "name was erased from {rendered:?}");
        assert!(rendered.contains('…'), "long alias was not elided in {rendered:?}");
    }

    #[test]
    fn fixed_preset_picker_width_does_not_feed_back_between_frames() {
        let mut preset = shell_preset_with_stale_ssh_metadata();
        preset.name = "deploy staging with a deliberately long preset name".to_string();
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let picker_width = 320.0;
        let mut widths = Vec::new();

        for _ in 0..4 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                let area = egui::Area::new(egui::Id::new("preset-picker-width-test")).show(ctx, |ui| {
                    ui.set_width(picker_width);
                    let _ =
                        render_grouped_preset_rows(ui, None, [0.0, 0.0], std::slice::from_ref(&preset), picker_width);
                });
                widths.push(area.response.rect.width());
            });
        }

        assert!(widths.iter().all(|width| (*width - picker_width).abs() < f32::EPSILON));
        assert!(widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < f32::EPSILON));
    }
}
