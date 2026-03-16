use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{RenderableContent, RenderableCursor, point_to_viewport};
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{CornerRadius, Pos2, Rect, StrokeKind, Vec2};

use crate::theme;

use super::layout::{GridMetrics, usize_to_f32};

#[profiling::function]
pub(super) fn render_grid(ui: &egui::Ui, rect: Rect, content: RenderableContent<'_>, metrics: &GridMetrics) {
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, CornerRadius::same(8), theme::PANEL_BG);

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

#[profiling::function]
pub(super) fn render_cursor(
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
