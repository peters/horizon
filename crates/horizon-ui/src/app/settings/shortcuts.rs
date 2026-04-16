use egui::{Color32, Ui};
use horizon_core::{Config, ShortcutBinding, ShortcutsConfig};

use crate::app::shortcut_inventory::{
    copy_selection_shortcut_label, paste_shortcut_label, ssh_reconnect_shortcut_conflicts,
};
use crate::app::util;
use crate::terminal_widget::SSH_RECONNECT_SHORTCUT;
use crate::theme;

const LABEL_WIDTH: f32 = 130.0;
const ERROR_INDICATOR_WIDTH: f32 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditableShortcut {
    CommandPalette,
    NewTerminal,
    FocusWorkspace,
    FitWorkspace,
    RemoteHosts,
    Sessions,
    ToggleSidebar,
    ToggleHud,
    ToggleMinimap,
    AlignWorkspaces,
    ToggleSettings,
    ResetZoom,
    ZoomIn,
    ZoomOut,
    FullscreenPanel,
    ExitFullscreen,
    FullscreenWindow,
    SaveEditor,
    SearchTerminals,
}

impl EditableShortcut {
    const ALL: [Self; 19] = [
        Self::CommandPalette,
        Self::NewTerminal,
        Self::FocusWorkspace,
        Self::FitWorkspace,
        Self::RemoteHosts,
        Self::Sessions,
        Self::ToggleSidebar,
        Self::ToggleHud,
        Self::ToggleMinimap,
        Self::AlignWorkspaces,
        Self::ToggleSettings,
        Self::ResetZoom,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::FullscreenPanel,
        Self::ExitFullscreen,
        Self::FullscreenWindow,
        Self::SaveEditor,
        Self::SearchTerminals,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::CommandPalette => "Command Palette",
            Self::NewTerminal => "New Terminal",
            Self::FocusWorkspace => "Focus Workspace",
            Self::FitWorkspace => "Fit Workspace",
            Self::RemoteHosts => "Remote Hosts",
            Self::Sessions => "Sessions",
            Self::ToggleSidebar => "Toggle Sidebar",
            Self::ToggleHud => "Toggle HUD",
            Self::ToggleMinimap => "Toggle Minimap",
            Self::AlignWorkspaces => "Align Workspaces",
            Self::ToggleSettings => "Toggle Settings",
            Self::ResetZoom => "Reset Zoom",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
            Self::FullscreenPanel => "Fullscreen Panel",
            Self::ExitFullscreen => "Exit Fullscreen",
            Self::FullscreenWindow => "Fullscreen Window",
            Self::SaveEditor => "Save Editor",
            Self::SearchTerminals => "Search Terminals",
        }
    }

    fn value_mut(self, shortcuts: &mut ShortcutsConfig) -> &mut String {
        match self {
            Self::CommandPalette => &mut shortcuts.command_palette,
            Self::NewTerminal => &mut shortcuts.new_terminal,
            Self::FocusWorkspace => &mut shortcuts.focus_active_workspace,
            Self::FitWorkspace => &mut shortcuts.fit_active_workspace,
            Self::RemoteHosts => &mut shortcuts.open_remote_hosts,
            Self::Sessions => &mut shortcuts.toggle_sessions,
            Self::ToggleSidebar => &mut shortcuts.toggle_sidebar,
            Self::ToggleHud => &mut shortcuts.toggle_hud,
            Self::ToggleMinimap => &mut shortcuts.toggle_minimap,
            Self::AlignWorkspaces => &mut shortcuts.align_workspaces_horizontally,
            Self::ToggleSettings => &mut shortcuts.toggle_settings,
            Self::ResetZoom => &mut shortcuts.zoom_reset,
            Self::ZoomIn => &mut shortcuts.zoom_in,
            Self::ZoomOut => &mut shortcuts.zoom_out,
            Self::FullscreenPanel => &mut shortcuts.fullscreen_panel,
            Self::ExitFullscreen => &mut shortcuts.exit_fullscreen_panel,
            Self::FullscreenWindow => &mut shortcuts.fullscreen_window,
            Self::SaveEditor => &mut shortcuts.save_editor,
            Self::SearchTerminals => &mut shortcuts.search,
        }
    }
}

/// Render the Shortcuts settings tab. Each configurable keyboard binding is
/// shown as an editable text field with inline validation. Returns `true`
/// when any binding changed.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;
    let mut all_valid = true;

    super::section_heading(ui, "Keyboard Shortcuts");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Key bindings use modifier+key syntax (e.g. Ctrl+K, Ctrl+Shift+A).");
        ui.add_space(8.0);

        for shortcut in EditableShortcut::ALL {
            changed |= shortcut_row(
                ui,
                shortcut.label(),
                shortcut.value_mut(&mut config.shortcuts),
                &mut all_valid,
            );
        }

        // Cross-field validation for duplicates/overlaps. Skip if any
        // individual binding failed to parse (those errors are shown inline
        // and resolve() would just repeat them).
        if all_valid && let Err(error) = config.shortcuts.resolve() {
            ui.add_space(8.0);
            if let horizon_core::Error::Config(msg) = &error {
                ui.label(egui::RichText::new(msg.as_str()).color(theme::PALETTE_RED()).size(11.0));
            }
        }
    });

    render_contextual_shortcuts(ui, config);

    changed
}

fn render_contextual_shortcuts(ui: &mut Ui, config: &Config) {
    let primary_label = util::primary_shortcut_label();
    let (reconnect_note, reconnect_note_color) = reconnect_shortcut_note(config);

    super::section_heading(ui, "Built-In Shortcuts");
    super::section_card(ui, |ui| {
        super::dim_label(ui, contextual_shortcuts_hint());
        ui.add_space(8.0);

        static_shortcut_row(
            ui,
            "Copy Selection",
            &copy_selection_shortcut_label(primary_label),
            None,
            theme::FG(),
        );
        static_shortcut_row(ui, "Paste", &paste_shortcut_label(primary_label), None, theme::FG());
        static_shortcut_row(
            ui,
            "Reconnect SSH Panel",
            &SSH_RECONNECT_SHORTCUT.display_label(primary_label),
            Some(reconnect_note),
            reconnect_note_color,
        );
    });
}

fn reconnect_shortcut_note(config: &Config) -> (&'static str, Color32) {
    match config.shortcuts.resolve() {
        Ok(shortcuts) if ssh_reconnect_shortcut_conflicts(&shortcuts) => (
            "Disabled while another global shortcut uses the same binding.",
            theme::PALETTE_RED(),
        ),
        Ok(_) => ("Available when a disconnected SSH panel is focused.", theme::FG_DIM()),
        Err(_) => (
            "Fix shortcut validation errors to confirm whether this shortcut is available.",
            theme::PALETTE_RED(),
        ),
    }
}

fn contextual_shortcuts_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "These shortcuts use macOS clipboard conventions and are shown here for reference; they are not editable."
    } else if cfg!(target_os = "windows") {
        "These shortcuts are shown here for reference; they are not editable. Plain Ctrl+C still reaches the terminal, and Windows also accepts Ctrl+Insert / Shift+Insert for copy and paste."
    } else {
        "These shortcuts are shown here for reference; they are not editable. Plain Ctrl+C still reaches the terminal, so copy uses Ctrl+Shift+C."
    }
}

fn shortcut_row(ui: &mut Ui, label: &str, value: &mut String, all_valid: &mut bool) -> bool {
    let validation = ShortcutBinding::parse(value);
    let text_color = if validation.is_ok() {
        theme::FG()
    } else {
        theme::PALETTE_RED()
    };
    let mut row_changed = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;

        let label_response = ui.label(egui::RichText::new(label).color(theme::FG_SOFT()).size(12.0));
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
            let indicator = ui.label(egui::RichText::new("!").color(theme::PALETTE_RED()).size(12.0).strong());
            indicator.on_hover_text(msg.as_str());
        } else if validation.is_err() {
            *all_valid = false;
        }
    });

    row_changed
}

fn static_shortcut_row(ui: &mut Ui, label: &str, value: &str, note: Option<&str>, note_color: Color32) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            let label_response = ui.label(egui::RichText::new(label).color(theme::FG_SOFT()).size(12.0));
            let used = label_response.rect.width();
            if used < LABEL_WIDTH {
                ui.add_space(LABEL_WIDTH - used);
            }

            ui.label(
                egui::RichText::new(value)
                    .color(theme::FG())
                    .size(12.0)
                    .family(egui::FontFamily::Monospace),
            );
        });

        if let Some(note) = note {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(LABEL_WIDTH);
                ui.label(egui::RichText::new(note).color(note_color).size(11.0));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use horizon_core::ShortcutsConfig;

    use super::EditableShortcut;

    #[test]
    fn editable_shortcuts_include_search_terminals() {
        assert!(EditableShortcut::ALL.contains(&EditableShortcut::SearchTerminals));
    }

    #[test]
    fn editable_shortcut_rows_update_the_expected_config_field() {
        let mut shortcuts = ShortcutsConfig::default();
        *EditableShortcut::SearchTerminals.value_mut(&mut shortcuts) = "Alt+F".to_string();

        assert_eq!(shortcuts.search, "Alt+F");
    }
}
