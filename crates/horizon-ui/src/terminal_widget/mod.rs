mod ime;
mod input;
mod layout;
mod render;
mod scrollbar;

use egui::{Context, FontId, Vec2};
use horizon_core::Panel;

use self::ime::{clear_terminal_ime_state, publish_terminal_ime_output};
pub(crate) use self::input::SSH_RECONNECT_SHORTCUT;
pub(crate) use self::input::TerminalSelectionDragState;
use self::input::{
    PointerSupport, handle_terminal_keyboard_input, handle_terminal_pointer_input, pty_mouse_reporting_enabled,
};
use self::layout::{GridMetrics, terminal_interaction, terminal_layout, terminal_viewport_size};
pub(crate) use self::render::TerminalGridCache;
use self::render::{render_cursor, render_grid};
use self::scrollbar::render_scrollbar;
use super::primary_selection::PrimarySelection;

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;

const fn hover_requires_grid_refresh(body_hovered: bool, mouse_reporting_enabled: bool) -> bool {
    body_hovered && mouse_reporting_enabled
}

const fn grid_cache_allowed(content_requires_refresh: bool, hover_requires_refresh: bool, drag_active: bool) -> bool {
    !(content_requires_refresh || hover_requires_refresh || drag_active)
}

fn terminal_has_keyboard_focus(
    ui: &egui::Ui,
    interaction: &layout::TerminalInteraction,
    is_active_panel: bool,
) -> bool {
    let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
    let other_widget_has_focus = ui
        .memory(egui::Memory::focused)
        .is_some_and(|focused| focused != interaction.body.id);
    window_focused && (interaction.body.has_focus() || (is_active_panel && !other_widget_has_focus))
}

const fn should_forward_terminal_keyboard_input(
    interactive: bool,
    has_terminal_focus: bool,
    requested_focus_this_frame: bool,
) -> bool {
    interactive && has_terminal_focus && !requested_focus_this_frame
}

pub struct TerminalView<'a> {
    panel: &'a mut Panel,
    grid_cache: Option<&'a mut TerminalGridCache>,
}

pub struct TerminalKeyboardContext<'a> {
    pub keyboard_events: &'a [super::input::TerminalInputEvent],
    pub primary_selection: &'a PrimarySelection,
    pub local_ssh_reconnect_enabled: bool,
    pub reconnect_requested: &'a mut bool,
}

impl<'a> TerminalView<'a> {
    pub fn new(panel: &'a mut Panel, grid_cache: Option<&'a mut TerminalGridCache>) -> Self {
        Self { panel, grid_cache }
    }

    /// Renders the terminal panel. Returns `true` if clicked (for focus tracking).
    #[profiling::function]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        is_active_panel: bool,
        interactive: bool,
        request_focus: bool,
        selection_drag: &mut TerminalSelectionDragState,
        keyboard: TerminalKeyboardContext<'_>,
    ) -> bool {
        let metrics = grid_metrics(ui.ctx());
        let char_width = metrics.char_width;
        let line_height = metrics.line_height;
        let layout = terminal_layout(ui.available_size(), char_width, line_height);
        let viewport = terminal_viewport_size(ui.available_size(), char_width, line_height);
        let new_cols = viewport.cols;
        let new_rows = viewport.rows;

        self.panel
            .resize(new_rows, new_cols, viewport.cell_width, viewport.cell_height);

        let interaction = terminal_interaction(ui, layout, self.panel.id.0, interactive);
        if interactive && request_focus {
            interaction.body.request_focus();
        }
        if interactive {
            handle_terminal_pointer_input(
                ui,
                self.panel,
                &interaction,
                is_active_panel,
                PointerSupport {
                    metrics: &metrics,
                    visible_rows: new_rows,
                    visible_cols: new_cols,
                    primary_selection: keyboard.primary_selection,
                    selection_drag,
                },
            );
        }
        let has_terminal_focus = terminal_has_keyboard_focus(ui, &interaction, is_active_panel);
        self.panel.set_focused(has_terminal_focus);

        if interactive && has_terminal_focus {
            ui.memory_mut(|mem| {
                mem.set_focus_lock_filter(
                    interaction.body.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                );
            });
            publish_terminal_ime_output(ui, self.panel, &interaction, &metrics);
        } else {
            clear_terminal_ime_state(ui, interaction.body.id);
        }

        let had_recent_output = self.panel.had_recent_output();
        let body_hovered = interaction.body.hovered();
        let modifiers = body_hovered.then(|| ui.input(|input| input.modifiers));
        let drag_active = interaction.body.dragged() || interaction.scrollbar.dragged();

        if ui.is_rect_visible(interaction.layout.outer)
            && let Some(terminal) = self.panel.terminal_mut()
        {
            let history_size = terminal.history_size();
            let scrollbar_highlighted = interaction.scrollbar.hovered() || interaction.scrollbar.dragged();
            let mut grid_cache = self.grid_cache.take();
            terminal.with_renderable_content(|content| {
                let cursor = content.cursor;
                let display_offset = content.display_offset;
                let mouse_reporting_enabled =
                    modifiers.is_some_and(|modifiers| pty_mouse_reporting_enabled(content.mode, modifiers));
                let hover_requires_refresh = hover_requires_grid_refresh(body_hovered, mouse_reporting_enabled);
                let content_requires_refresh = had_recent_output || content.selection.is_some();
                let allow_grid_cache =
                    grid_cache_allowed(content_requires_refresh, hover_requires_refresh, drag_active);
                render_grid(
                    ui,
                    interaction.layout.body,
                    content,
                    &metrics,
                    grid_cache.as_deref_mut(),
                    allow_grid_cache,
                );
                render_cursor(
                    ui,
                    interaction.layout.body,
                    cursor,
                    display_offset,
                    &metrics,
                    has_terminal_focus,
                );
                render_scrollbar(
                    ui,
                    interaction.layout.scrollbar,
                    display_offset,
                    usize::from(new_rows),
                    history_size,
                    scrollbar_highlighted,
                );
            });
            self.grid_cache = grid_cache;
        }

        if should_forward_terminal_keyboard_input(interactive, has_terminal_focus, request_focus) {
            *keyboard.reconnect_requested |= handle_terminal_keyboard_input(
                ui,
                interaction.body.id,
                self.panel,
                keyboard.keyboard_events,
                keyboard.primary_selection,
                keyboard.local_ssh_reconnect_enabled,
            );
        }

        interaction.body.clicked()
    }
}

fn grid_metrics(ctx: &Context) -> GridMetrics {
    let font_id = FontId::monospace(FONT_SIZE);
    let char_width = ctx.fonts_mut(|fonts| fonts.glyph_width(&font_id, 'M'));
    let line_height = FONT_SIZE * LINE_HEIGHT_FACTOR;

    GridMetrics {
        char_width,
        line_height,
        font_id,
    }
}

pub(crate) fn viewport_for_available_space(ctx: &Context, available: Vec2) -> layout::TerminalViewportSize {
    let metrics = grid_metrics(ctx);
    terminal_viewport_size(available, metrics.char_width, metrics.line_height)
}

#[cfg(test)]
mod tests {
    use egui::{Context, RawInput, Rect, Vec2};

    use super::{
        grid_cache_allowed, hover_requires_grid_refresh, should_forward_terminal_keyboard_input,
        terminal_has_keyboard_focus, terminal_interaction, terminal_layout,
    };

    #[test]
    fn mouse_reporting_hover_bypasses_grid_cache() {
        assert!(hover_requires_grid_refresh(true, true));
        assert!(!grid_cache_allowed(false, true, false));
    }

    #[test]
    fn normal_terminal_hover_keeps_grid_cache() {
        assert!(!hover_requires_grid_refresh(true, false));
        assert!(!hover_requires_grid_refresh(false, true));
        assert!(grid_cache_allowed(false, false, false));
    }

    #[test]
    fn dynamic_content_and_drags_bypass_grid_cache() {
        assert!(!grid_cache_allowed(true, false, false));
        assert!(!grid_cache_allowed(false, false, true));
    }

    #[test]
    fn requested_terminal_focus_replaces_an_old_widget_focus_for_keyboard_routing() {
        let ctx = Context::default();
        let mut requested_body_id = None;
        let _ = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(640.0, 480.0))),
                ..RawInput::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let old = ui.button("old focus");
                    old.request_focus();
                    let interaction =
                        terminal_interaction(ui, terminal_layout(Vec2::new(400.0, 240.0), 8.0, 17.0), 7, true);
                    assert!(!terminal_has_keyboard_focus(ui, &interaction, true));

                    interaction.body.request_focus();
                    requested_body_id = Some(interaction.body.id);
                    assert!(terminal_has_keyboard_focus(ui, &interaction, true));
                });
            },
        );

        assert_eq!(ctx.memory(egui::Memory::focused), requested_body_id);
    }

    #[test]
    fn focus_transfer_drops_the_activation_frames_keyboard_input() {
        assert!(!should_forward_terminal_keyboard_input(true, true, true));
        assert!(should_forward_terminal_keyboard_input(true, true, false));
    }
}
