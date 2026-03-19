use egui::Ui;
use horizon_core::{Config, ShortcutBinding};

use crate::theme;

/// Render the Shortcuts settings tab.  Each keyboard binding is shown as
/// an editable text field with inline validation.  Returns `true` when
/// any binding changed.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    let mut all_valid = true;

    super::section_heading(ui, "Keyboard Shortcuts");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Key bindings use modifier+key syntax (e.g. Ctrl+K, Ctrl+Shift+A).");
        ui.add_space(8.0);

        egui::Grid::new("settings_shortcuts_grid")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .min_col_width(0.0)
            .show(ui, |ui| {
                changed |= shortcut_row(
                    ui,
                    "Command Palette",
                    &mut config.shortcuts.command_palette,
                    &mut all_valid,
                );
                changed |= shortcut_row(ui, "New Terminal", &mut config.shortcuts.new_terminal, &mut all_valid);
                changed |= shortcut_row(
                    ui,
                    "Remote Hosts",
                    &mut config.shortcuts.open_remote_hosts,
                    &mut all_valid,
                );
                changed |= shortcut_row(
                    ui,
                    "Toggle Sidebar",
                    &mut config.shortcuts.toggle_sidebar,
                    &mut all_valid,
                );
                changed |= shortcut_row(ui, "Toggle HUD", &mut config.shortcuts.toggle_hud, &mut all_valid);
                changed |= shortcut_row(
                    ui,
                    "Toggle Minimap",
                    &mut config.shortcuts.toggle_minimap,
                    &mut all_valid,
                );
                changed |= shortcut_row(
                    ui,
                    "Align Workspaces",
                    &mut config.shortcuts.align_workspaces_horizontally,
                    &mut all_valid,
                );
                changed |= shortcut_row(
                    ui,
                    "Toggle Settings",
                    &mut config.shortcuts.toggle_settings,
                    &mut all_valid,
                );
                changed |= shortcut_row(ui, "Reset View", &mut config.shortcuts.reset_view, &mut all_valid);
                changed |= shortcut_row(ui, "Zoom In", &mut config.shortcuts.zoom_in, &mut all_valid);
                changed |= shortcut_row(ui, "Zoom Out", &mut config.shortcuts.zoom_out, &mut all_valid);
                changed |= shortcut_row(
                    ui,
                    "Fullscreen Panel",
                    &mut config.shortcuts.fullscreen_panel,
                    &mut all_valid,
                );
                changed |= shortcut_row(
                    ui,
                    "Exit Fullscreen",
                    &mut config.shortcuts.exit_fullscreen_panel,
                    &mut all_valid,
                );
                changed |= shortcut_row(
                    ui,
                    "Fullscreen Window",
                    &mut config.shortcuts.fullscreen_window,
                    &mut all_valid,
                );
                changed |= shortcut_row(ui, "Save Editor", &mut config.shortcuts.save_editor, &mut all_valid);
            });

        // Cross-field validation for duplicates/overlaps.  Skip if any
        // individual binding failed to parse (those errors are shown inline
        // and resolve() would just repeat them).
        if all_valid && let Err(error) = config.shortcuts.resolve() {
            ui.add_space(8.0);
            if let horizon_core::Error::Config(msg) = &error {
                ui.label(egui::RichText::new(msg.as_str()).color(theme::PALETTE_RED).size(11.0));
            }
        }
    });

    changed
}

fn shortcut_row(ui: &mut Ui, label: &str, value: &mut String, all_valid: &mut bool) -> bool {
    let validation = ShortcutBinding::parse(value);
    let text_color = if validation.is_ok() {
        theme::FG
    } else {
        theme::PALETTE_RED
    };

    ui.label(egui::RichText::new(label).color(theme::FG_SOFT).size(12.0));

    let response = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(140.0)
            .font(egui::FontId::monospace(12.0))
            .text_color(text_color),
    );

    // Error indicator column: show a tooltip with the parse error.
    if let Err(horizon_core::Error::Config(msg)) = &validation {
        *all_valid = false;
        let indicator = ui.label(egui::RichText::new("!").color(theme::PALETTE_RED).size(12.0).strong());
        indicator.on_hover_text(msg.as_str());
    } else if validation.is_err() {
        *all_valid = false;
        ui.label("");
    } else {
        // Empty cell to keep the grid aligned.
        ui.label("");
    }

    ui.end_row();
    response.changed()
}
