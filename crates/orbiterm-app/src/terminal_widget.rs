use std::collections::VecDeque;

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{RenderableContent, RenderableCursor, point_to_viewport};
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{CornerRadius, FontId, Key, Pos2, Rect, StrokeKind, Vec2};
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

        if ui.is_rect_visible(interaction.layout.outer) {
            let history_size = self.panel.terminal.history_size();
            let scrollbar_highlighted = interaction.scrollbar.hovered() || interaction.scrollbar.dragged();
            self.panel.terminal.with_renderable_content(|content| {
                let cursor = content.cursor;
                let display_offset = content.display_offset;
                render_grid(ui, interaction.layout.body, content, &metrics);
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
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) {
    if interaction.body.clicked() {
        interaction.body.request_focus();
    }
    if is_active_panel && ui.input(|input| input.key_pressed(Key::Tab)) {
        interaction.body.request_focus();
    }

    let terminal_mode = panel.terminal.mode();
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let pointer_buttons = ui.input(|input| input::PointerButtons {
        primary: input.pointer.primary_down(),
        middle: input.pointer.middle_down(),
        secondary: input.pointer.secondary_down(),
    });
    let current_modifiers = ui.input(|input| input.modifiers);
    let hovered_point = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|position| interaction.layout.body.contains(*position))
        .and_then(|position| {
            grid_point_from_position(interaction.layout.body, position, metrics, visible_rows, visible_cols)
        });

    for event in &events {
        match event {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            } if interaction.layout.body.contains(*pos) => {
                if *pressed {
                    interaction.body.request_focus();
                }
                if let Some(point) =
                    grid_point_from_position(interaction.layout.body, *pos, metrics, visible_rows, visible_cols)
                    && let Some(bytes) = input::mouse_button_report(*button, *pressed, *modifiers, terminal_mode, point)
                    && !bytes.is_empty()
                {
                    panel.write_input(&bytes);
                }
            }
            egui::Event::PointerMoved(pos) if interaction.layout.body.contains(*pos) => {
                if let Some(point) =
                    grid_point_from_position(interaction.layout.body, *pos, metrics, visible_rows, visible_cols)
                    && let Some(bytes) =
                        input::mouse_motion_report(pointer_buttons, current_modifiers, terminal_mode, point)
                    && !bytes.is_empty()
                {
                    panel.write_input(&bytes);
                }
            }
            egui::Event::MouseWheel { delta, modifiers, .. } => {
                if let Some(point) = hovered_point
                    && let Some(action) = input::wheel_action(
                        *delta,
                        Vec2::new(metrics.char_width, metrics.line_height),
                        *modifiers,
                        terminal_mode,
                        point,
                    )
                {
                    match action {
                        input::WheelAction::Pty(bytes) if !bytes.is_empty() => panel.write_input(&bytes),
                        input::WheelAction::Pty(_) => {}
                        input::WheelAction::Scrollback(lines) => panel.scroll_scrollback_by(lines),
                    }
                }
            }
            _ => {}
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
                panel.terminal.history_size(),
            ),
            panel.terminal.history_size(),
        );
        panel.set_scrollback(target_scrollback);
    }
}

fn handle_terminal_keyboard_input(ui: &egui::Ui, panel: &mut Panel) {
    let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
    let mode = panel.terminal.mode();
    let mut suppressed_text = VecDeque::new();

    for event in &events {
        match event {
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if suppressed_text.front().is_some_and(|expected| expected == text) {
                    suppressed_text.pop_front();
                } else {
                    panel.write_input(text.as_bytes());
                }
            }
            egui::Event::Paste(text) => {
                let bytes = input::paste_bytes(text, mode, true);
                panel.write_input(&bytes);
            }
            egui::Event::Copy => panel.write_input(&[3]),
            egui::Event::Cut => panel.write_input(&[24]),
            egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                if let Some(translation) = input::translate_key_event(*key, *pressed, *repeat, *modifiers, mode) {
                    if let Some(text) = translation.suppress_text {
                        suppressed_text.push_back(text);
                    }
                    if !translation.bytes.is_empty() {
                        panel.write_input(&translation.bytes);
                    }
                }
            }
            _ => {}
        }
    }
}

fn render_grid(ui: &egui::Ui, rect: Rect, content: RenderableContent<'_>, metrics: &GridMetrics) {
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, CornerRadius::same(6), theme::PANEL_BG);

    for indexed in content.display_iter {
        let Some(point) = point_to_viewport(content.display_offset, indexed.point) else {
            continue;
        };
        let x = rect.min.x + usize_to_f32(point.column.0) * metrics.char_width;
        let y = rect.min.y + usize_to_f32(point.line) * metrics.line_height;
        let width = if indexed.cell.flags.contains(Flags::WIDE_CHAR) {
            metrics.char_width * 2.0
        } else {
            metrics.char_width
        };
        let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, metrics.line_height));
        let selected = content
            .selection
            .is_some_and(|selection| selection.contains_cell(&indexed, indexed.point, content.cursor.shape));
        let (fg, bg) = cell_colors(indexed.cell, selected, content.colors);

        if bg != theme::PANEL_BG || selected {
            painter.rect_filled(cell_rect, CornerRadius::ZERO, bg);
        }

        if let Some(text) = cell_text(indexed.cell)
            && !text.is_empty()
        {
            painter.text(
                Pos2::new(x, y),
                egui::Align2::LEFT_TOP,
                text,
                metrics.font_id.clone(),
                fg,
            );
        }

        paint_cell_decoration(&painter, cell_rect, indexed.cell, content.colors, fg);
    }
}

fn render_cursor(
    ui: &egui::Ui,
    rect: Rect,
    cursor: RenderableCursor,
    display_offset: usize,
    metrics: &GridMetrics,
    has_focus: bool,
) {
    if cursor.shape == CursorShape::Hidden {
        return;
    }

    let Some(point) = point_to_viewport(display_offset, cursor.point) else {
        return;
    };
    let x = rect.min.x + usize_to_f32(point.column.0) * metrics.char_width;
    let y = rect.min.y + usize_to_f32(point.line) * metrics.line_height;
    let cursor_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(metrics.char_width, metrics.line_height));
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.2, theme::CURSOR.gamma_multiply(0.82));

    if !has_focus {
        painter.rect_stroke(cursor_rect, CornerRadius::same(1), stroke, StrokeKind::Outside);
        return;
    }

    match cursor.shape {
        CursorShape::Block => {
            painter.rect_filled(cursor_rect, CornerRadius::same(1), theme::CURSOR.gamma_multiply(0.8));
        }
        CursorShape::Underline => {
            let underline = Rect::from_min_size(
                Pos2::new(cursor_rect.min.x, cursor_rect.max.y - 2.0),
                Vec2::new(cursor_rect.width(), 2.0),
            );
            painter.rect_filled(underline, CornerRadius::same(1), theme::CURSOR.gamma_multiply(0.9));
        }
        CursorShape::Beam => {
            let beam = Rect::from_min_size(cursor_rect.min, Vec2::new(2.0, cursor_rect.height()));
            painter.rect_filled(beam, CornerRadius::same(1), theme::CURSOR.gamma_multiply(0.9));
        }
        CursorShape::HollowBlock => {
            painter.rect_stroke(cursor_rect, CornerRadius::same(1), stroke, StrokeKind::Outside);
        }
        CursorShape::Hidden => {}
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
    painter.rect_filled(rect, CornerRadius::same(u8::MAX), track_fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(u8::MAX),
        egui::Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 180)),
        StrokeKind::Outside,
    );

    let thumb_height = scrollbar_thumb_height(rect.height(), quantize_visible_rows(visible_rows), scrollback_limit);
    let thumb_rect = scrollbar_thumb_rect(rect, thumb_height, scrollback, scrollback_limit);
    painter.rect_filled(
        thumb_rect,
        CornerRadius::same(u8::MAX),
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

fn cell_colors(
    cell: &Cell,
    selected: bool,
    colors: &alacritty_terminal::term::color::Colors,
) -> (egui::Color32, egui::Color32) {
    let mut fg = theme::terminal_color_to_egui(cell.fg, colors);
    let mut bg = theme::terminal_color_to_egui(cell.bg, colors);

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.flags.contains(Flags::DIM) {
        fg = fg.gamma_multiply(0.82);
    }

    if cell.flags.contains(Flags::HIDDEN) {
        fg = bg;
    }

    if selected {
        std::mem::swap(&mut fg, &mut bg);
        bg = theme::alpha(theme::ACCENT, 76);
        fg = theme::FG;
    }

    (fg, bg)
}

fn cell_text(cell: &Cell) -> Option<String> {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
    {
        return None;
    }

    if cell.c == ' ' && cell.zerowidth().is_none() {
        return None;
    }

    let mut text = String::new();
    text.push(cell.c);
    if let Some(chars) = cell.zerowidth() {
        for ch in chars {
            text.push(*ch);
        }
    }

    Some(text)
}

fn paint_cell_decoration(
    painter: &egui::Painter,
    cell_rect: Rect,
    cell: &Cell,
    colors: &alacritty_terminal::term::color::Colors,
    color: egui::Color32,
) {
    if cell.flags.intersects(
        Flags::UNDERLINE
            | Flags::DOUBLE_UNDERLINE
            | Flags::UNDERCURL
            | Flags::DOTTED_UNDERLINE
            | Flags::DASHED_UNDERLINE,
    ) {
        let underline_color = cell
            .underline_color()
            .map_or(color, |underline| theme::terminal_color_to_egui(underline, colors));
        let y = cell_rect.max.y - 1.5;
        painter.line_segment(
            [Pos2::new(cell_rect.min.x, y), Pos2::new(cell_rect.max.x, y)],
            egui::Stroke::new(1.0, underline_color),
        );
    }

    if cell.flags.contains(Flags::STRIKEOUT) {
        let y = cell_rect.center().y;
        painter.line_segment(
            [Pos2::new(cell_rect.min.x, y), Pos2::new(cell_rect.max.x, y)],
            egui::Stroke::new(1.0, color),
        );
    }
}

fn grid_point_from_position(
    rect: Rect,
    position: Pos2,
    metrics: &GridMetrics,
    visible_rows: u16,
    visible_cols: u16,
) -> Option<input::GridPoint> {
    if !rect.contains(position) {
        return None;
    }

    let relative = position - rect.min;
    let row = (relative.y / metrics.line_height).floor().max(0.0);
    let column = (relative.x / metrics.char_width).floor().max(0.0);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        Some(input::GridPoint {
            line: (row as usize).min(usize::from(visible_rows.saturating_sub(1))),
            column: (column as usize).min(usize::from(visible_cols.saturating_sub(1))),
        })
    }
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

fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

fn f32_to_usize(value: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as usize
    }
}
