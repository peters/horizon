mod clipboard_paste;
mod ime;
mod input;
mod layout;
mod link_notice;
mod notice_strip;
mod program_end_notice;
mod render;
mod scrollbar;

use egui::{Context, FontId, Vec2};
use horizon_core::Panel;

use self::ime::{clear_terminal_ime_state, publish_terminal_ime_output};
pub(crate) use self::input::SSH_RECONNECT_SHORTCUT;
pub(crate) use self::input::TerminalSelectionDragState;
use self::input::{PointerSupport, handle_terminal_keyboard_input, handle_terminal_pointer_input};
use self::layout::{GridMetrics, terminal_interaction, terminal_layout, terminal_viewport_size};
use self::link_notice::LinkNotice;
pub(crate) use self::program_end_notice::PanelNoticeAction;
pub(crate) use self::render::TerminalGridCache;
use self::render::{render_cursor, render_grid};
use self::scrollbar::render_scrollbar;
use super::primary_selection::PrimarySelection;

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;

pub struct TerminalView<'a> {
    panel: &'a mut Panel,
    grid_cache: Option<&'a mut TerminalGridCache>,
    /// Canvas points at the bottom of the available area that the scrollbar
    /// leaves to the panel's corner resize grip.
    scrollbar_bottom_clearance: f32,
}

pub struct TerminalKeyboardContext<'a> {
    pub keyboard_events: &'a [super::input::TerminalInputEvent],
    pub primary_selection: &'a PrimarySelection,
    /// Clipboard images become a private file whose path the terminal pastes.
    pub clipboard_images: &'a super::clipboard_image::ClipboardImages,
    pub local_ssh_reconnect_enabled: bool,
    /// What the person pressed in the line under the grid of this panel, which
    /// the frame that draws the panel reads once.
    pub notice_action: &'a mut PanelNoticeAction,
    /// The person created or revealed this panel, so its body takes the keyboard
    /// of the window and the next key reaches its program.
    pub take_keyboard_focus: bool,
}

impl<'a> TerminalView<'a> {
    pub fn new(
        panel: &'a mut Panel,
        grid_cache: Option<&'a mut TerminalGridCache>,
        scrollbar_bottom_clearance: f32,
    ) -> Self {
        Self {
            panel,
            grid_cache,
            scrollbar_bottom_clearance,
        }
    }

    /// Renders the terminal panel. Returns `true` if clicked (for focus tracking).
    #[profiling::function]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        is_active_panel: bool,
        interactive: bool,
        selection_drag: &mut TerminalSelectionDragState,
        keyboard: TerminalKeyboardContext<'_>,
    ) -> bool {
        let metrics = grid_metrics(ui.ctx());
        let char_width = metrics.char_width;
        clipboard_paste::keep_frames(ui.ctx());
        let line_height = metrics.line_height;
        let layout = terminal_layout(
            ui.available_size(),
            char_width,
            line_height,
            self.scrollbar_bottom_clearance,
        );
        let viewport = terminal_viewport_size(ui.available_size(), char_width, line_height);
        // A panel whose grid is not the one of its own body keeps that grid: the
        // widget never asks it to resize, it draws the grid from the top left of
        // the body, and the pointer and the scrollbar read that grid. A follow
        // panel shows the grid of its stream, and a panel whose program a follower
        // holds shows the grid of that follower.
        let streamed = self.panel.terminal().filter(|terminal| terminal.shows_foreign_grid());
        let (new_rows, new_cols) = streamed.map_or((viewport.rows, viewport.cols), |terminal| terminal.shown_grid());
        if streamed.is_none() {
            self.panel
                .resize(new_rows, new_cols, viewport.cell_width, viewport.cell_height);
        }
        // The body of a follow panel gives the grid that its stream asks for when
        // the person types in it.
        if let Some(feed) = self.panel.follow_feed() {
            feed.set_fit(viewport.cols, viewport.rows);
        }

        let interaction = terminal_interaction(ui, layout, self.panel.id.0, interactive);
        if interactive && keyboard.take_keyboard_focus {
            // The person asked for this panel, so its body takes the keys of the
            // window even when another widget held them.
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
        let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
        let other_widget_has_focus = ui
            .memory(egui::Memory::focused)
            .is_some_and(|focused| focused != interaction.body.id);
        let has_terminal_focus =
            window_focused && (interaction.body.has_focus() || (is_active_panel && !other_widget_has_focus));
        self.panel.set_focused(has_terminal_focus);

        if interactive && has_terminal_focus {
            let target_id = egui::Id::unique(("native_image_paste_target", ui.ctx().viewport_id()));
            ui.ctx()
                .data_mut(|data| data.insert_temp(target_id, (self.panel.id, interaction.body.id)));
        }

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
        let drag_active = interaction.body.dragged() || interaction.scrollbar.dragged();

        let on_screen = ui.is_rect_visible(interaction.layout.outer);
        // The echo of a key press asks for a frame only for a panel that a
        // frame draws, so a person who cannot see the panel never makes the
        // window repaint for its output.
        self.panel.note_drawn(on_screen);
        if on_screen && let Some(terminal) = self.panel.terminal_mut() {
            let history_size = terminal.history_size();
            let scrollbar_highlighted = interaction.scrollbar.hovered() || interaction.scrollbar.dragged();
            let mut grid_cache = self.grid_cache.take();
            terminal.with_renderable_content(|content| {
                let cursor = content.cursor;
                let display_offset = content.display_offset;
                // Pointer reporting does not change grid pixels. Output and selection
                // refresh the grid; cursor and scrollbar feedback are painted separately.
                let allow_grid_cache = !had_recent_output && content.selection.is_none() && !drag_active;
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
            // The grid of this frame carries whatever output the frame drained,
            // so the latency line of a key press that this grid echoes ends here.
            self.panel.note_output_painted();
        }

        // The line of a panel that has no shell sits under the grid, so a person
        // reads why the keys do nothing and where the next attempt starts. Only a
        // panel in that state builds it; a connected terminal has no such work.
        self.show_panel_notice(ui, interaction.layout.body, interactive, &mut *keyboard.notice_action);

        if interactive && has_terminal_focus {
            // The bytes of a key press arm the echo of this panel, so the first
            // output after them asks for the next frame instead of waiting for
            // the idle poll of the window. Output that nobody typed arms nothing,
            // so a program that prints without end never drives the window.
            let reconnect_requested = handle_terminal_keyboard_input(
                ui,
                interaction.body.id,
                self.panel,
                keyboard.keyboard_events,
                keyboard.primary_selection,
                keyboard.clipboard_images,
                keyboard.local_ssh_reconnect_enabled,
            );
            if reconnect_requested {
                *keyboard.notice_action = PanelNoticeAction::Reconnect;
            }
        }

        interaction.body.clicked()
    }

    /// Paints the line of a panel that holds no shell, and takes the control that
    /// the person presses in it: `Reconnect` for a lost link, `Start again` and
    /// `Close` for a program that ended.
    fn show_panel_notice(
        &self,
        ui: &mut egui::Ui,
        body: egui::Rect,
        interactive: bool,
        action: &mut PanelNoticeAction,
    ) {
        if let Some(connection) = self.panel.ssh_connection.as_ref()
            && let Some(notice) = LinkNotice::of(self.panel.kind, self.panel.ssh_status(), &connection.display_label())
        {
            if link_notice::show(ui, body, &notice, interactive) {
                *action = PanelNoticeAction::Reconnect;
            }
            return;
        }
        if let Some(line) = self.panel.follow_ended_line() {
            // Only a new screen of the stream brings a follow panel back, so the
            // line of a stream that ended has no control.
            notice_strip::ended_line(ui, body, &line);
            return;
        }
        if let Some(end) = self.panel.program_end() {
            let pressed = program_end_notice::ProgramEndNotice { end }.show(ui, body, interactive);
            if pressed != PanelNoticeAction::None {
                *action = pressed;
            }
        }
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

/// The cell size the terminal widget converts wheel deltas into lines with.
pub(crate) fn wheel_cell_size(ctx: &Context) -> Vec2 {
    let metrics = grid_metrics(ctx);
    Vec2::new(metrics.char_width, metrics.line_height)
}

pub(crate) fn viewport_for_available_space(ctx: &Context, available: Vec2) -> layout::TerminalViewportSize {
    let metrics = grid_metrics(ctx);
    terminal_viewport_size(available, metrics.char_width, metrics.line_height)
}

/// The scrollbar's hit rectangle, in the terminal's own coordinate space, for a
/// terminal that leaves `scrollbar_bottom_clearance` free at its bottom.
#[cfg(test)]
pub(crate) fn scrollbar_hit_rect(available: Vec2, scrollbar_bottom_clearance: f32) -> egui::Rect {
    let metrics = GridMetrics {
        char_width: FONT_SIZE * 0.6,
        line_height: FONT_SIZE * LINE_HEIGHT_FACTOR,
        font_id: FontId::monospace(FONT_SIZE),
    };
    let layout = terminal_layout(
        available,
        metrics.char_width,
        metrics.line_height,
        scrollbar_bottom_clearance,
    );
    layout.scrollbar.expand2(Vec2::splat(layout::SCROLLBAR_HIT_PADDING))
}

// The regression fixture uses a Unix shell and PTY.
#[cfg(all(test, unix))]
mod cache_tests;

// A follow panel holds a terminal on a Unix socket pair.
#[cfg(all(test, unix))]
mod follow_tests;
