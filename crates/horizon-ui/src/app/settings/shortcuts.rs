use egui::Ui;
use horizon_core::{Config, ShortcutBinding};

use crate::theme;

const LABEL_WIDTH: f32 = 130.0;
const ERROR_INDICATOR_WIDTH: f32 = 20.0;

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

        changed |= shortcut_row(
            ui,
            "Command Palette",
            &mut config.shortcuts.command_palette,
            &mut all_valid,
        );
        changed |= shortcut_row(ui, "New Terminal", &mut config.shortcuts.new_terminal, &mut all_valid);
        changed |= shortcut_row(
            ui,
            "Focus Workspace",
            &mut config.shortcuts.focus_active_workspace,
            &mut all_valid,
        );
        changed |= shortcut_row(
            ui,
            "Fit Workspace",
            &mut config.shortcuts.fit_active_workspace,
            &mut all_valid,
        );
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
    let mut row_changed = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;

        let label_response = ui.label(egui::RichText::new(label).color(theme::FG_SOFT).size(12.0));
        let used = label_response.rect.width();
        if used < LABEL_WIDTH {
            ui.add_space(LABEL_WIDTH - used);
        }

        let input_width = (ui.available_width() - ERROR_INDICATOR_WIDTH).max(60.0);
        row_changed = ui
            .add(
                egui::TextEdit::singleline(value)
                    .desired_width(input_width)
                    .font(egui::FontId::monospace(12.0))
                    .text_color(text_color),
            )
            .changed();

        if let Err(horizon_core::Error::Config(msg)) = &validation {
            *all_valid = false;
            let indicator = ui.label(egui::RichText::new("!").color(theme::PALETTE_RED).size(12.0).strong());
            indicator.on_hover_text(msg.as_str());
        } else if validation.is_err() {
            *all_valid = false;
        }
    });

    row_changed
}
