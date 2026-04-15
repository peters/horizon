use egui::{Color32, Stroke, Ui};
use horizon_core::{Config, PanelKind, PanelResume, PresetConfig};

use crate::theme;

const ALL_KINDS: [PanelKind; 11] = [
    PanelKind::Shell,
    PanelKind::Ssh,
    PanelKind::Codex,
    PanelKind::Claude,
    PanelKind::OpenCode,
    PanelKind::Gemini,
    PanelKind::KiloCode,
    PanelKind::Command,
    PanelKind::Editor,
    PanelKind::GitChanges,
    PanelKind::Usage,
];

/// Render the Presets settings tab.  Returns `true` when the preset list
/// or any individual preset was modified.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;

    super::section_heading(ui, "Panel Presets");

    // Header row: description + add button
    ui.horizontal(|ui| {
        super::dim_label(ui, "Templates for quickly creating new panels via the command palette.");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("+ Add Preset").size(11.0).color(theme::ACCENT()))
                        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.08))
                        .stroke(Stroke::new(
                            1.0,
                            theme::blend(theme::BORDER_SUBTLE(), theme::ACCENT(), 0.3),
                        ))
                        .corner_radius(8),
                )
                .clicked()
            {
                config.presets.push(PresetConfig {
                    name: "New Preset".to_string(),
                    alias: None,
                    kind: PanelKind::Shell,
                    command: None,
                    args: Vec::new(),
                    resume: PanelResume::Fresh,
                    ssh_connection: None,
                });
                changed = true;
            }
        });
    });
    ui.add_space(8.0);

    // Preset cards
    let mut remove_index: Option<usize> = None;
    for (index, preset) in config.presets.iter_mut().enumerate() {
        changed |= render_preset_card(ui, index, preset, &mut remove_index);
        ui.add_space(6.0);
    }

    if let Some(index) = remove_index {
        config.presets.remove(index);
        changed = true;
    }

    changed
}

fn render_preset_card(ui: &mut Ui, index: usize, preset: &mut PresetConfig, remove_index: &mut Option<usize>) -> bool {
    let mut changed = false;
    let has_error = preset_has_error(preset);

    let border_color = if has_error {
        theme::blend(theme::BORDER_SUBTLE(), theme::PALETTE_RED(), 0.5)
    } else {
        theme::BORDER_SUBTLE()
    };

    egui::Frame::default()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, border_color))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            // Row 1: name, alias, delete button
            ui.horizontal(|ui| {
                let name_color = if preset.name.trim().is_empty() {
                    theme::PALETTE_RED()
                } else {
                    theme::FG()
                };
                let name_response = ui.add(
                    egui::TextEdit::singleline(&mut preset.name)
                        .desired_width(140.0)
                        .hint_text("Name")
                        .font(egui::FontId::proportional(12.0))
                        .text_color(name_color),
                );
                if preset.name.trim().is_empty() {
                    name_response.on_hover_text("Preset name cannot be empty");
                }

                let mut alias_str = preset.alias.clone().unwrap_or_default();
                let alias_response = ui.add(
                    egui::TextEdit::singleline(&mut alias_str)
                        .desired_width(60.0)
                        .hint_text("alias")
                        .font(egui::FontId::monospace(11.0)),
                );
                if alias_response.changed() {
                    preset.alias = if alias_str.is_empty() { None } else { Some(alias_str) };
                    changed = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("X").size(11.0).color(theme::PALETTE_RED()))
                                .fill(Color32::TRANSPARENT)
                                .corner_radius(4),
                        )
                        .clicked()
                    {
                        *remove_index = Some(index);
                    }
                });
            });

            // Row 2: kind and resume dropdowns
            ui.horizontal(|ui| {
                changed |= render_kind_combo(ui, index, &mut preset.kind);
                ui.add_space(12.0);
                changed |= render_resume_combo(ui, index, &mut preset.resume);
            });

            // SSH validation error
            if preset.kind == PanelKind::Ssh
                && let Some(ref conn) = preset.ssh_connection
                && !conn.is_valid()
            {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("SSH host cannot be empty")
                        .color(theme::PALETTE_RED())
                        .size(11.0),
                );
            }
        });

    changed
}

fn preset_has_error(preset: &PresetConfig) -> bool {
    if preset.name.trim().is_empty() {
        return true;
    }
    if preset.kind == PanelKind::Ssh
        && let Some(ref conn) = preset.ssh_connection
        && !conn.is_valid()
    {
        return true;
    }
    false
}

fn render_kind_combo(ui: &mut Ui, index: usize, kind: &mut PanelKind) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Kind").color(theme::FG_DIM()).size(11.0));
    let kind_id = egui::Id::new("preset_kind").with(index);
    egui::ComboBox::from_id_salt(kind_id)
        .selected_text(kind.display_name())
        .width(100.0)
        .show_ui(ui, |ui| {
            for k in ALL_KINDS {
                if ui.selectable_value(kind, k, k.display_name()).changed() {
                    changed = true;
                }
            }
        });
    changed
}

fn render_resume_combo(ui: &mut Ui, index: usize, resume: &mut PanelResume) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Resume").color(theme::FG_DIM()).size(11.0));
    let resume_id = egui::Id::new("preset_resume").with(index);
    let resume_label = match resume {
        PanelResume::Fresh => "Fresh",
        PanelResume::Last => "Last",
        PanelResume::Session { .. } => "Session",
    };
    egui::ComboBox::from_id_salt(resume_id)
        .selected_text(resume_label)
        .width(80.0)
        .show_ui(ui, |ui| {
            if ui.selectable_value(resume, PanelResume::Fresh, "Fresh").changed() {
                changed = true;
            }
            if ui.selectable_value(resume, PanelResume::Last, "Last").changed() {
                changed = true;
            }
        });
    changed
}
