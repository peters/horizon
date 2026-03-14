use egui::{FontId, Key, Pos2, Rect, Rounding, Vec2};
use orbiterm_core::Panel;

use crate::input;
use crate::theme;

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT_FACTOR: f32 = 1.3;
const SCROLLBAR_WIDTH: f32 = 12.0;
const SCROLLBAR_GAP: f32 = 8.0;
const SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 18.0;

pub struct TerminalView<'a> {
    panel: &'a mut Panel,
}

impl<'a> TerminalView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
    }

    /// Renders the terminal panel. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, is_active_panel: bool) -> bool {
        let font_id = FontId::monospace(FONT_SIZE);
        let char_width = ui.fonts(|f| f.glyph_width(&font_id, 'M'));
        let line_height = FONT_SIZE * LINE_HEIGHT_FACTOR;
        let layout = terminal_layout(ui.available_size(), char_width, line_height);
        let new_cols = quantize_dimension(layout.body.width() / char_width);
        let new_rows = quantize_dimension(layout.body.height() / line_height);
        let metrics = GridMetrics {
            char_width,
            line_height,
            font_id,
        };

        if new_cols != self.panel.terminal.cols() || new_rows != self.panel.terminal.rows() {
            self.panel.resize(new_rows, new_cols);
        }
        let interaction = terminal_interaction(ui, layout, self.panel.id.0);
        handle_terminal_pointer_input(ui, self.panel, &interaction, is_active_panel, line_height, new_rows);
        let other_widget_has_focus = ui
            .memory(egui::Memory::focused)
            .is_some_and(|focused| focused != interaction.body.id);
        let has_terminal_focus = interaction.body.has_focus() || (is_active_panel && !other_widget_has_focus);

        if ui.is_rect_visible(interaction.layout.outer) {
            let screen = self.panel.terminal.screen();
            render_grid(ui, interaction.layout.body, screen, &metrics);
            render_cursor(ui, interaction.layout.body, screen, &metrics, has_terminal_focus);
            render_scrollbar(
                ui,
                interaction.layout.scrollbar,
                self.panel.terminal.scrollback(),
                usize::from(new_rows),
                self.panel.terminal.scrollback_limit(),
                interaction.scrollbar.hovered() || interaction.scrollbar.dragged(),
            );
        }

        if has_terminal_focus {
            handle_terminal_keyboard_input(ui, self.panel);
        }

        interaction.body.clicked()
    }
}

struct GridMetrics {
    char_width: f32,
    line_height: f32,
    font_id: FontId,
}

#[derive(Clone, Copy)]
struct TerminalLayout {
    outer: Rect,
    body: Rect,
    scrollbar: Rect,
}

struct TerminalInteraction {
    layout: TerminalLayout,
    body: egui::Response,
    scrollbar: egui::Response,
}

fn terminal_layout(available: Vec2, char_width: f32, line_height: f32) -> TerminalLayout {
    let body_size = Vec2::new(
        (available.x - SCROLLBAR_WIDTH - SCROLLBAR_GAP).max(char_width),
        available.y.max(line_height),
    );
    let outer = Rect::from_min_size(
        Pos2::ZERO,
        Vec2::new(body_size.x + SCROLLBAR_WIDTH + SCROLLBAR_GAP, body_size.y),
    );
    let body = Rect::from_min_size(outer.min, body_size);
    let scrollbar = Rect::from_min_size(
        Pos2::new(body.max.x + SCROLLBAR_GAP, outer.min.y + 4.0),
        Vec2::new(SCROLLBAR_WIDTH, (outer.height() - 8.0).max(24.0)),
    );

    TerminalLayout { outer, body, scrollbar }
}

fn terminal_interaction(ui: &mut egui::Ui, layout: TerminalLayout, panel_id: u64) -> TerminalInteraction {
    let (allocated_rect, _) = ui.allocate_exact_size(layout.outer.size(), egui::Sense::hover());
    let layout = TerminalLayout {
        outer: allocated_rect,
        body: layout.body.translate(allocated_rect.min.to_vec2()),
        scrollbar: layout.scrollbar.translate(allocated_rect.min.to_vec2()),
    };
    let body = ui.interact(
        layout.body,
        ui.make_persistent_id(("terminal_body", panel_id)),
        egui::Sense::click(),
    );
    let scrollbar = ui.interact(
        layout.scrollbar.expand2(Vec2::new(2.0, 2.0)),
        ui.make_persistent_id(("terminal_scrollbar", panel_id)),
        egui::Sense::click_and_drag(),
    );

    TerminalInteraction {
        layout,
        body,
        scrollbar,
    }
}

fn handle_terminal_pointer_input(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    interaction: &TerminalInteraction,
    is_active_panel: bool,
    line_height: f32,
    visible_rows: u16,
) {
    if interaction.body.clicked() {
        interaction.body.request_focus();
    }
    if is_active_panel && ui.input(|input| input.key_pressed(Key::Tab)) {
        interaction.body.request_focus();
    }
    if interaction.body.hovered() {
        let scroll_delta_y = ui.input(|input| input.smooth_scroll_delta.y + input.raw_scroll_delta.y);
        let scroll_lines = scroll_lines_from_delta(scroll_delta_y, line_height);
        if scroll_lines != 0 {
            panel.scroll_scrollback_by(scroll_lines);
        }
    }
    if (interaction.scrollbar.dragged() || interaction.scrollbar.clicked())
        && let Some(pointer_position) = ui.input(|input| input.pointer.interact_pos())
    {
        let target_scrollback = scrollbar_pointer_to_scrollback(
            pointer_position,
            interaction.scrollbar.rect.shrink2(Vec2::new(2.0, 2.0)),
            scrollbar_thumb_height(
                interaction.scrollbar.rect.height() - 4.0,
                visible_rows,
                panel.terminal.scrollback_limit(),
            ),
            panel.terminal.scrollback_limit(),
        );
        panel.set_scrollback(target_scrollback);
    }
}

fn handle_terminal_keyboard_input(ui: &egui::Ui, panel: &mut Panel) {
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    for event in &events {
        match event {
            egui::Event::Text(text) | egui::Event::Paste(text) => panel.write_input(text.as_bytes()),
            egui::Event::Copy => panel.write_input(&[3]),
            egui::Event::Cut => panel.write_input(&[24]),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.ctrl && modifiers.shift {
                    continue;
                }
                if let Some(bytes) = input::key_to_bytes(*key, *modifiers) {
                    panel.write_input(&bytes);
                }
            }
            _ => {}
        }
    }
}

fn render_grid(ui: &egui::Ui, rect: Rect, screen: &vt100::Screen, metrics: &GridMetrics) {
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, Rounding::same(6.0), theme::PANEL_BG);

    let (rows, cols) = screen.size();

    for row in 0..rows {
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                let x = rect.min.x + f32::from(col) * metrics.char_width;
                let y = rect.min.y + f32::from(row) * metrics.line_height;

                let bg = theme::vt100_color_to_egui(cell.bgcolor(), false);
                if bg != theme::PANEL_BG {
                    let cell_rect =
                        Rect::from_min_size(Pos2::new(x, y), Vec2::new(metrics.char_width, metrics.line_height));
                    painter.rect_filled(cell_rect, 0.0, bg);
                }

                let contents = cell.contents();
                if !contents.is_empty() && contents != " " {
                    let fg = theme::vt100_color_to_egui(cell.fgcolor(), true);
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        &contents,
                        metrics.font_id.clone(),
                        fg,
                    );
                }
            }
        }
    }
}

fn render_cursor(ui: &egui::Ui, rect: Rect, screen: &vt100::Screen, metrics: &GridMetrics, has_focus: bool) {
    let (cursor_row, cursor_col) = screen.cursor_position();
    let cx = rect.min.x + f32::from(cursor_col) * metrics.char_width;
    let cy = rect.min.y + f32::from(cursor_row) * metrics.line_height;
    let cursor_rect = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(metrics.char_width, metrics.line_height));

    let painter = ui.painter_at(rect);

    if has_focus {
        // Solid block cursor when focused
        painter.rect_filled(cursor_rect, Rounding::same(1.0), theme::CURSOR.gamma_multiply(0.8));
    } else {
        // Hollow rectangle when not focused
        painter.rect_stroke(
            cursor_rect,
            Rounding::same(1.0),
            egui::Stroke::new(1.0, theme::CURSOR.gamma_multiply(0.4)),
        );
    }
}

fn render_scrollbar(
    ui: &egui::Ui,
    rect: Rect,
    scrollback: usize,
    visible_rows: usize,
    scrollback_limit: usize,
    highlighted: bool,
) {
    let painter = ui.painter_at(rect.expand2(Vec2::new(2.0, 0.0)));
    let track_fill = if highlighted {
        theme::alpha(theme::PANEL_BG_ALT, 220)
    } else {
        theme::alpha(theme::PANEL_BG_ALT, 170)
    };
    painter.rect_filled(rect, Rounding::same(999.0), track_fill);
    painter.rect_stroke(
        rect,
        Rounding::same(999.0),
        egui::Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 180)),
    );

    let thumb_height = scrollbar_thumb_height(rect.height(), quantize_visible_rows(visible_rows), scrollback_limit);
    let thumb_rect = scrollbar_thumb_rect(rect, thumb_height, scrollback, scrollback_limit);
    painter.rect_filled(
        thumb_rect,
        Rounding::same(999.0),
        if scrollback > 0 || highlighted {
            theme::alpha(theme::ACCENT, 210)
        } else {
            theme::alpha(theme::FG_DIM, 140)
        },
    );
}

fn scrollbar_thumb_height(track_height: f32, visible_rows: u16, scrollback_limit: usize) -> f32 {
    let visible_rows = f32::from(visible_rows.max(1));
    let total_rows = visible_rows + usize_to_f32(scrollback_limit.max(1));
    (track_height * (visible_rows / total_rows)).clamp(SCROLLBAR_MIN_THUMB_HEIGHT, track_height)
}

fn scrollbar_thumb_rect(track_rect: Rect, thumb_height: f32, scrollback: usize, scrollback_limit: usize) -> Rect {
    let max_scrollback = usize_to_f32(scrollback_limit.max(1));
    let scroll_ratio = (usize_to_f32(scrollback).min(max_scrollback) / max_scrollback).clamp(0.0, 1.0);
    let travel = (track_rect.height() - thumb_height).max(0.0);
    let thumb_top = track_rect.max.y - thumb_height - (travel * scroll_ratio);

    Rect::from_min_size(
        Pos2::new(track_rect.min.x + 1.0, thumb_top),
        Vec2::new((track_rect.width() - 2.0).max(4.0), thumb_height),
    )
}

fn scrollbar_pointer_to_scrollback(
    pointer_position: Pos2,
    track_rect: Rect,
    thumb_height: f32,
    scrollback_limit: usize,
) -> usize {
    let clamped_y = pointer_position.y.clamp(track_rect.min.y, track_rect.max.y);
    let travel = (track_rect.height() - thumb_height).max(1.0);
    let relative = (track_rect.max.y - thumb_height - clamped_y).clamp(0.0, travel);
    let ratio = (relative / travel).clamp(0.0, 1.0);
    f32_to_usize((ratio * usize_to_f32(scrollback_limit.max(1))).round())
}

fn quantize_dimension(value: f32) -> u16 {
    let clamped = if value.is_finite() {
        value.floor().clamp(1.0, f32::from(u16::MAX))
    } else {
        1.0
    };

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        clamped as u16
    }
}

fn quantize_visible_rows(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn scroll_lines_from_delta(scroll_delta_y: f32, line_height: f32) -> i32 {
    if !scroll_delta_y.is_finite() || !line_height.is_finite() || line_height <= 0.0 {
        return 0;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    {
        (scroll_delta_y / line_height)
            .round()
            .clamp(i32::MIN as f32, i32::MAX as f32) as i32
    }
}

fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

fn f32_to_usize(value: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as usize
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_lines_from_delta;

    #[test]
    fn scroll_delta_maps_to_terminal_lines() {
        assert_eq!(scroll_lines_from_delta(26.0, 13.0), 2);
        assert_eq!(scroll_lines_from_delta(-26.0, 13.0), -2);
        assert_eq!(scroll_lines_from_delta(3.0, 13.0), 0);
        assert_eq!(scroll_lines_from_delta(5.0, 0.0), 0);
    }
}
