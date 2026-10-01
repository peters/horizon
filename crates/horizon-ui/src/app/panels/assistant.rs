//! Draws the assistant agent's terminal inside the drawer instead of on the canvas.

use egui::Ui;
use horizon_core::PanelId;

use super::{PanelBodyContext, show_panel_body_contents};
use crate::app::HorizonApp;
use crate::app::shortcut_inventory::global_shortcut_bindings;

impl HorizonApp {
    /// Returns whether the terminal body was clicked, which claims the keyboard.
    pub(in crate::app) fn show_assistant_terminal(&mut self, ui: &mut Ui, panel_id: PanelId) -> bool {
        let raw_events = ui.ctx().input(|input| input.events.clone());
        self.terminal_keyboard_events = self.terminal_events_for_viewport(ui.ctx(), &raw_events);
        let local_ssh_reconnect_enabled = self.local_ssh_reconnect_shortcut_enabled();
        let all_shortcut_bindings = global_shortcut_bindings(&self.shortcuts);
        let is_focused = self.assistant_has_keyboard() && !self.speech_text_surface_active().0;
        let interactive = !self.host_dialog_open();
        let mut reconnect_requested = false;
        let Some(panel) = self.board.panel_mut(panel_id) else {
            return false;
        };
        let clicked = show_panel_body_contents(
            ui,
            panel,
            is_focused,
            interactive,
            PanelBodyContext {
                keyboard_events: &self.terminal_keyboard_events,
                browser_events: &[],
                editor_save_shortcut: self.shortcuts.save_editor,
                editor_preview_cache: None,
                local_ssh_reconnect_enabled,
                primary_selection: &self.primary_selection,
                reconnect_requested: &mut reconnect_requested,
                terminal_selection_drag: &mut self.terminal_selection_drag,
                terminal_grid_cache: Some(
                    self.panel_render_caches
                        .terminal_grid_cache
                        .entry(panel_id)
                        .or_default(),
                ),
                browser_ui_state: None,
                device_ui_state: None,
                browser_shortcuts: None,
                browser_shortcut_bindings: &all_shortcut_bindings,
                browser_frame_has_pointer_button: false,
                browser_fullscreen_active: false,
                browser_zoom_wheel_owner: crate::browser_widget::ZoomWheelOwner::Page,
            },
        );
        if reconnect_requested {
            self.queue_panel_restart(panel_id);
        }
        clicked
    }
}
