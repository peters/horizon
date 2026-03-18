mod input;
mod layout;
mod render;
mod scrollbar;

use alacritty_terminal::term::TermMode;
use egui::FontId;
use horizon_core::Panel;

use self::input::{handle_terminal_keyboard_input, handle_terminal_pointer_input};
use self::layout::{GridMetrics, quantize_dimension, terminal_interaction, terminal_layout};
pub(crate) use self::render::TerminalGridCache;
use self::render::{render_cursor, render_grid};
use self::scrollbar::render_scrollbar;

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;

pub struct TerminalView<'a> {
    panel: &'a mut Panel,
    grid_cache: Option<&'a mut TerminalGridCache>,
}

impl<'a> TerminalView<'a> {
    pub fn new(panel: &'a mut Panel, grid_cache: Option<&'a mut TerminalGridCache>) -> Self {
        Self { panel, grid_cache }
    }

    /// Renders the terminal panel. Returns `true` if clicked (for focus tracking).
    #[profiling::function]
    pub fn show(&mut self, ui: &mut egui::Ui, is_active_panel: bool) -> bool {
        let font_id = FontId::monospace(FONT_SIZE);
        let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font_id, 'M'));
        let line_height = FONT_SIZE * LINE_HEIGHT_FACTOR;
        let layout = terminal_layout(ui.available_size(), char_width, line_height);
        let new_cols = quantize_dimension(layout.body.width() / char_width).max(2);
        let new_rows = quantize_dimension(layout.body.height() / line_height);
        let metrics = GridMetrics {
            char_width,
            line_height,
            font_id,
        };

        self.panel.resize(
            new_rows,
            new_cols,
            quantize_dimension(char_width),
            quantize_dimension(line_height),
        );

        let interaction = terminal_interaction(ui, layout, self.panel.id.0);
        handle_terminal_pointer_input(
            ui,
            self.panel,
            &interaction,
            is_active_panel,
            &metrics,
            new_rows,
            new_cols,
        );
        let window_focused = ui.input(|input| input.viewport().focused.unwrap_or(true));
        let other_widget_has_focus = ui
            .memory(egui::Memory::focused)
            .is_some_and(|focused| focused != interaction.body.id);
        let has_terminal_focus =
            window_focused && (interaction.body.has_focus() || (is_active_panel && !other_widget_has_focus));
        self.panel.set_focused(has_terminal_focus);

        if has_terminal_focus {
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
        }

        let allow_grid_cache = !self.panel.had_recent_output()
            && self.panel.terminal().is_some_and(|terminal| !terminal.has_selection())
            && !interaction.body.dragged()
            && !interaction.scrollbar.dragged();
        let allow_sparse_frame_reuse = self.panel.had_recent_output()
            && self
                .panel
                .terminal()
                .is_some_and(|terminal| !terminal.has_selection() && terminal.mode().contains(TermMode::ALT_SCREEN))
            && !interaction.body.dragged()
            && !interaction.scrollbar.dragged();

        if ui.is_rect_visible(interaction.layout.outer)
            && let Some(terminal) = self.panel.terminal_mut()
        {
            let history_size = terminal.history_size();
            let scrollbar_highlighted = interaction.scrollbar.hovered() || interaction.scrollbar.dragged();
            let mut grid_cache = self.grid_cache.take();
            terminal.with_renderable_content(|content| {
                let cursor = content.cursor;
                let display_offset = content.display_offset;
                render_grid(
                    ui,
                    interaction.layout.body,
                    content,
                    &metrics,
                    grid_cache.as_deref_mut(),
                    allow_grid_cache,
                    allow_sparse_frame_reuse,
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

        if has_terminal_focus {
            handle_terminal_keyboard_input(ui, self.panel);
        }

        interaction.body.clicked()
    }
}
