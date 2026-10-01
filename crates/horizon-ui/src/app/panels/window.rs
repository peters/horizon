//! A panel as a native window of its own (desktop-workspace prototype).
//!
//! The window holds only the panel's body. The desktop draws the title bar, moves,
//! resizes and tiles it like any other application, and lets the person put it on
//! another desktop workspace.

use egui::{Align, Event, Layout, Ui, UiBuilder, ViewportCommand};
use horizon_core::{PanelId, PanelKind};

use super::{HorizonApp, PanelBodyContext, show_panel_body_contents};
use crate::app::shortcut_inventory::global_shortcut_bindings;
use crate::theme;

impl HorizonApp {
    pub(in crate::app) fn render_panel_window(&mut self, ui: &mut Ui, panel_id: PanelId) {
        let ctx = ui.ctx().clone();
        if ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.close_panel(panel_id);
            return;
        }
        let Some(kind) = self.board.panel(panel_id).map(|panel| panel.kind) else {
            return;
        };
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        if focused && self.board.focused != Some(panel_id) {
            self.board.focus(panel_id);
        }
        let raw_events = ctx.input(|input| input.events.clone());
        self.terminal_keyboard_events = self.terminal_events_for_viewport(&ctx, &raw_events);
        let browser_events: Vec<Event> = if kind == PanelKind::Browser {
            self.terminal_keyboard_events
                .iter()
                .map(|input| input.event.clone())
                .collect()
        } else {
            Vec::new()
        };
        let frame_has_pointer_button = browser_events
            .iter()
            .any(|event| matches!(event, Event::PointerButton { .. }));
        let local_ssh_reconnect_enabled = self.local_ssh_reconnect_shortcut_enabled();
        let browser_shortcuts = (kind == PanelKind::Browser).then(|| self.shortcuts.clone());
        let shortcut_bindings = global_shortcut_bindings(&self.shortcuts);
        let interactive = !self.host_dialog_open();
        let claim_focus = focused && !self.speech_text_surface_active().0;
        let mut clicked = false;
        let mut reconnect_requested = false;

        ui.painter().rect_filled(ui.max_rect(), 0.0, theme::BG());
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(ui.max_rect())
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                let board = &mut self.board;
                let caches = &mut self.panel_render_caches;
                let Some(panel) = board.panel_mut(panel_id) else {
                    return;
                };
                let preview_cache =
                    (panel.kind == PanelKind::Editor).then(|| caches.editor_preview_cache.entry(panel_id).or_default());
                let grid_cache = panel
                    .terminal()
                    .is_some()
                    .then(|| caches.terminal_grid_cache.entry(panel_id).or_default());
                let browser_state =
                    (panel.kind == PanelKind::Browser).then(|| caches.browser_ui_state.entry(panel_id).or_default());
                let device_state =
                    (panel.kind == PanelKind::Device).then(|| caches.device_ui_state.entry(panel_id).or_default());
                clicked = show_panel_body_contents(
                    ui,
                    panel,
                    claim_focus,
                    interactive,
                    PanelBodyContext {
                        keyboard_events: &self.terminal_keyboard_events,
                        browser_events: &browser_events,
                        editor_save_shortcut: self.shortcuts.save_editor,
                        editor_preview_cache: preview_cache,
                        local_ssh_reconnect_enabled,
                        primary_selection: &self.primary_selection,
                        reconnect_requested: &mut reconnect_requested,
                        terminal_selection_drag: &mut self.terminal_selection_drag,
                        terminal_grid_cache: grid_cache,
                        browser_ui_state: browser_state,
                        device_ui_state: device_state,
                        browser_shortcuts: browser_shortcuts.as_ref(),
                        browser_shortcut_bindings: &shortcut_bindings,
                        browser_frame_has_pointer_button: frame_has_pointer_button,
                        browser_fullscreen_active: false,
                        browser_zoom_wheel_owner: crate::browser_widget::ZoomWheelOwner::Page,
                    },
                );
            },
        );
        if clicked {
            self.board.focus(panel_id);
        }
        if reconnect_requested {
            self.queue_panel_restart(panel_id);
        }
    }
}
