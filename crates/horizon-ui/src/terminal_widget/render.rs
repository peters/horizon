use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{RenderableContent, RenderableCursor, point_to_viewport};
use alacritty_terminal::vte::ansi::CursorShape;
use alacritty_terminal::vte::ansi::{Color as TerminalColor, NamedColor};
use egui::{Color32, CornerRadius, Pos2, Rect, Shape, StrokeKind, Vec2};

use crate::theme;

use super::layout::{GridMetrics, usize_to_f32};

struct TextRun {
    line: usize,
    next_column: usize,
    x: f32,
    y: f32,
    fg: Color32,
    text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridCacheKey {
    rect_min_x_bits: u32,
    rect_min_y_bits: u32,
    rect_width_bits: u32,
    rect_height_bits: u32,
    char_width_bits: u32,
    line_height_bits: u32,
    display_offset: usize,
}

impl GridCacheKey {
    fn new(rect: Rect, display_offset: usize, metrics: &GridMetrics) -> Self {
        Self {
            rect_min_x_bits: rect.min.x.to_bits(),
            rect_min_y_bits: rect.min.y.to_bits(),
            rect_width_bits: rect.width().to_bits(),
            rect_height_bits: rect.height().to_bits(),
            char_width_bits: metrics.char_width.to_bits(),
            line_height_bits: metrics.line_height.to_bits(),
            display_offset,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct TerminalGridCache {
    key: Option<GridCacheKey>,
    shapes: Vec<Shape>,
}

#[profiling::function]
pub(super) fn render_grid(
    ui: &egui::Ui,
    rect: Rect,
    content: RenderableContent<'_>,
    metrics: &GridMetrics,
    grid_cache: Option<&mut TerminalGridCache>,
    allow_grid_cache: bool,
) {
    let painter = ui.painter_at(rect);
    let key = GridCacheKey::new(rect, content.display_offset, metrics);

    if let Some(grid_cache) = grid_cache {
        if allow_grid_cache && grid_cache.key == Some(key) {
            painter.extend(grid_cache.shapes.iter().cloned());
            return;
        }

        let shapes = build_grid_shapes(ui, rect, content, metrics);
        painter.extend(shapes.iter().cloned());
        grid_cache.key = Some(key);
        grid_cache.shapes = shapes;
        return;
    }

    painter.extend(build_grid_shapes(ui, rect, content, metrics));
}

fn build_grid_shapes(ui: &egui::Ui, rect: Rect, content: RenderableContent<'_>, metrics: &GridMetrics) -> Vec<Shape> {
    let mut shapes = Vec::new();

    ui.fonts_mut(|fonts| {
        let mut text_run: Option<TextRun> = None;

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
            let batchable_char = batchable_cell_char(indexed.cell, selected)
                .filter(|_| bg == theme::PANEL_BG && !has_cell_decoration(indexed.cell));

            if let Some(ch) = batchable_char {
                let can_continue = text_run
                    .as_ref()
                    .is_some_and(|run| run.line == point.line && run.next_column == point.column.0 && run.fg == fg);

                if can_continue {
                    if let Some(run) = &mut text_run {
                        run.text.push(ch);
                        run.next_column += 1;
                    }
                    continue;
                }

                flush_text_run(fonts, &mut shapes, metrics, &mut text_run);
                if ch != ' ' {
                    let mut text = String::with_capacity(64);
                    text.push(ch);
                    text_run = Some(TextRun {
                        line: point.line,
                        next_column: point.column.0 + 1,
                        x,
                        y,
                        fg,
                        text,
                    });
                }
                continue;
            }

            flush_text_run(fonts, &mut shapes, metrics, &mut text_run);

            if bg != theme::PANEL_BG || selected {
                shapes.push(Shape::rect_filled(cell_rect, CornerRadius::ZERO, bg));
            }

            if let Some(text) = cell_text(indexed.cell)
                && !text.is_empty()
            {
                shapes.push(Shape::text(
                    fonts,
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    metrics.font_id.clone(),
                    fg,
                ));
            }

            append_cell_decoration(&mut shapes, cell_rect, indexed.cell, content.colors, fg);
        }

        flush_text_run(fonts, &mut shapes, metrics, &mut text_run);
    });

    shapes
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
    let style_flags = Flags::INVERSE | Flags::DIM | Flags::HIDDEN;
    if !selected
        && !cell.flags.intersects(style_flags)
        && matches!(
            cell.fg,
            TerminalColor::Named(NamedColor::Foreground | NamedColor::BrightForeground)
        )
        && matches!(cell.bg, TerminalColor::Named(NamedColor::Background))
    {
        return (theme::FG, theme::PANEL_BG);
    }

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

fn batchable_cell_char(cell: &Cell, selected: bool) -> Option<char> {
    if selected
        || cell
            .flags
            .intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
        || cell.zerowidth().is_some()
    {
        return None;
    }

    Some(cell.c)
}

fn flush_text_run(
    fonts: &mut egui::epaint::text::FontsView<'_>,
    shapes: &mut Vec<Shape>,
    metrics: &GridMetrics,
    run: &mut Option<TextRun>,
) {
    let Some(run) = run.take() else {
        return;
    };
    if run.text.is_empty() {
        return;
    }

    shapes.push(Shape::text(
        fonts,
        Pos2::new(run.x, run.y),
        egui::Align2::LEFT_TOP,
        run.text,
        metrics.font_id.clone(),
        run.fg,
    ));
}

fn has_cell_decoration(cell: &Cell) -> bool {
    cell.flags.intersects(
        Flags::UNDERLINE
            | Flags::DOUBLE_UNDERLINE
            | Flags::UNDERCURL
            | Flags::DOTTED_UNDERLINE
            | Flags::DASHED_UNDERLINE
            | Flags::STRIKEOUT,
    )
}

fn append_cell_decoration(
    shapes: &mut Vec<Shape>,
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
        shapes.push(Shape::line_segment(
            [Pos2::new(cell_rect.min.x, y), Pos2::new(cell_rect.max.x, y)],
            egui::Stroke::new(1.0, underline_color),
        ));
    }

    if cell.flags.contains(Flags::STRIKEOUT) {
        let y = cell_rect.center().y;
        shapes.push(Shape::line_segment(
            [Pos2::new(cell_rect.min.x, y), Pos2::new(cell_rect.max.x, y)],
            egui::Stroke::new(1.0, color),
        ));
    }
}
