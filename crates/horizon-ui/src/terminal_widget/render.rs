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

struct BackgroundRun {
    line: usize,
    next_column: usize,
    rect: Rect,
    bg: Color32,
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

impl TerminalGridCache {
    fn invalidate(&mut self) {
        self.key = None;
        self.shapes.clear();
    }
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
    let has_selection = content.selection.is_some();

    if let Some(grid_cache) = grid_cache {
        if allow_grid_cache && grid_cache.key == Some(key) {
            painter.extend(grid_cache.shapes.iter().cloned());
            return;
        }

        let shapes = build_grid_shapes(ui, rect, content, metrics);
        if has_selection {
            // Copy/cut clear the model selection outside this render pass, and
            // the cache key tracks only geometry and scroll offset. Retaining
            // highlighted shapes would replay a stale selection highlight on
            // the next cache-eligible frame. Nothing outlives this pass, so the
            // shapes move straight into the painter instead of being cloned.
            painter.extend(shapes);
            grid_cache.invalidate();
        } else {
            painter.extend(shapes.iter().cloned());
            grid_cache.key = Some(key);
            grid_cache.shapes = shapes;
        }
        return;
    }

    painter.extend(build_grid_shapes(ui, rect, content, metrics));
}

fn build_grid_shapes(ui: &egui::Ui, rect: Rect, content: RenderableContent<'_>, metrics: &GridMetrics) -> Vec<Shape> {
    // Text runs can legitimately span cells whose background changes later in the line.
    // Keep backgrounds in a separate layer so those later fills never paint over glyphs.
    let mut background_shapes = Vec::new();
    let mut foreground_shapes = Vec::new();

    ui.fonts_mut(|fonts| {
        let mut text_run: Option<TextRun> = None;
        let mut background_run: Option<BackgroundRun> = None;

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
                .is_some_and(|selection| selection.contains_cell(&indexed, content.cursor.point, content.cursor.shape));
            let (fg, bg) = cell_colors(indexed.cell, selected, content.colors);
            let batchable_char = batchable_cell_char(indexed.cell).filter(|_| !has_cell_decoration(indexed.cell));

            if cell_is_spacer(indexed.cell) {
                flush_background_run(&mut background_shapes, &mut background_run);
            } else {
                append_background_rect(
                    &mut background_shapes,
                    &mut background_run,
                    point.line,
                    point.column.0,
                    cell_rect,
                    bg,
                    selected,
                );
            }

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

                flush_text_run(fonts, &mut foreground_shapes, metrics, &mut text_run);
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

            flush_text_run(fonts, &mut foreground_shapes, metrics, &mut text_run);

            if let Some(text) = cell_text(indexed.cell)
                && !text.is_empty()
            {
                foreground_shapes.push(Shape::text(
                    fonts,
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    metrics.font_id.clone(),
                    fg,
                ));
            }

            append_cell_decoration(&mut foreground_shapes, cell_rect, indexed.cell, content.colors, fg);
        }

        flush_text_run(fonts, &mut foreground_shapes, metrics, &mut text_run);
        flush_background_run(&mut background_shapes, &mut background_run);
    });

    merge_shape_layers(background_shapes, foreground_shapes)
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
    let stroke = egui::Stroke::new(1.2_f32, theme::CURSOR().gamma_multiply(0.82));

    if !has_focus {
        painter.rect_stroke(cursor_rect, CornerRadius::same(1), stroke, StrokeKind::Outside);
        return;
    }

    match cursor.shape {
        CursorShape::Block => {
            painter.rect_filled(cursor_rect, CornerRadius::same(1), theme::CURSOR().gamma_multiply(0.8));
        }
        CursorShape::Underline => {
            let underline = Rect::from_min_size(
                Pos2::new(cursor_rect.min.x, cursor_rect.max.y - 2.0),
                Vec2::new(cursor_rect.width(), 2.0),
            );
            painter.rect_filled(underline, CornerRadius::same(1), theme::CURSOR().gamma_multiply(0.9));
        }
        CursorShape::Beam => {
            let beam = Rect::from_min_size(cursor_rect.min, Vec2::new(2.0, cursor_rect.height()));
            painter.rect_filled(beam, CornerRadius::same(1), theme::CURSOR().gamma_multiply(0.9));
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
        return (theme::FG(), theme::PANEL_BG());
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
        bg = theme::alpha(theme::ACCENT(), 76);
        fg = theme::FG();
    }

    if bg.a() < u8::MAX {
        bg = theme::composite_over(theme::PANEL_BG(), bg);
    }
    if fg.a() < u8::MAX {
        fg = theme::composite_over(bg, fg);
    }

    if !cell.flags.contains(Flags::HIDDEN) {
        fg = theme::ensure_terminal_text_contrast(fg, bg);
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

    let zerowidth = cell.zerowidth();
    if cell.c == ' ' && zerowidth.is_none() {
        return None;
    }

    match zerowidth {
        Some(chars) => {
            let mut text = String::with_capacity(cell.c.len_utf8() + chars.len() * 3);
            text.push(cell.c);
            for ch in chars {
                text.push(*ch);
            }
            Some(text)
        }
        None => Some(cell.c.to_string()),
    }
}

fn batchable_cell_char(cell: &Cell) -> Option<char> {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
        || cell.zerowidth().is_some()
    {
        return None;
    }

    Some(cell.c)
}
fn cell_is_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
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

fn append_background_rect(
    shapes: &mut Vec<Shape>,
    run: &mut Option<BackgroundRun>,
    line: usize,
    column: usize,
    cell_rect: Rect,
    bg: Color32,
    selected: bool,
) {
    if bg == theme::PANEL_BG() && !selected {
        flush_background_run(shapes, run);
        return;
    }

    let can_continue = run
        .as_ref()
        .is_some_and(|current| current.line == line && current.next_column == column && current.bg == bg);
    if can_continue {
        if let Some(current) = run {
            current.rect.max.x = cell_rect.max.x;
            current.next_column = column + 1;
        }
        return;
    }

    flush_background_run(shapes, run);
    *run = Some(BackgroundRun {
        line,
        next_column: column + 1,
        rect: cell_rect,
        bg,
    });
}

fn flush_background_run(shapes: &mut Vec<Shape>, run: &mut Option<BackgroundRun>) {
    let Some(run) = run.take() else {
        return;
    };
    shapes.push(Shape::rect_filled(run.rect, CornerRadius::ZERO, run.bg));
}

fn merge_shape_layers(mut background_shapes: Vec<Shape>, foreground_shapes: Vec<Shape>) -> Vec<Shape> {
    background_shapes.extend(foreground_shapes);
    background_shapes
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
            egui::Stroke::new(1.0_f32, underline_color),
        ));
    }

    if cell.flags.contains(Flags::STRIKEOUT) {
        let y = cell_rect.center().y;
        shapes.push(Shape::line_segment(
            [Pos2::new(cell_rect.min.x, y), Pos2::new(cell_rect.max.x, y)],
            egui::Stroke::new(1.0_f32, color),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{GridMetrics, TerminalGridCache, cell_colors, merge_shape_layers, render_grid};
    use crate::theme;
    use alacritty_terminal::event::{Event as PtyEvent, EventListener};
    use alacritty_terminal::grid::{Dimensions, Indexed};
    use alacritty_terminal::index::{Column, Line, Point, Side};
    use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
    use alacritty_terminal::term::cell::Cell;
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::term::{Config as TermConfig, Term};
    use alacritty_terminal::vte::ansi::{self, Color as TerminalColor, CursorShape, NamedColor};
    use egui::{Color32, FontId, Pos2, Rect, Shape, Vec2};

    struct NoopProxy;

    impl EventListener for NoopProxy {
        fn send_event(&self, _event: PtyEvent) {}
    }

    struct TestGridSize;

    impl Dimensions for TestGridSize {
        fn total_lines(&self) -> usize {
            4
        }

        fn screen_lines(&self) -> usize {
            4
        }

        fn columns(&self) -> usize {
            12
        }
    }

    fn test_term_with_text(text: &str) -> Term<NoopProxy> {
        let mut term = Term::new(TermConfig::default(), &TestGridSize, NoopProxy);
        let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    fn render_grid_pass(
        ctx: &egui::Context,
        term: &mut Term<NoopProxy>,
        metrics: &GridMetrics,
        cache: &mut TerminalGridCache,
        allow_grid_cache: bool,
    ) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(96.0, 64.0));
                render_grid(
                    ui,
                    rect,
                    term.renderable_content(),
                    metrics,
                    Some(cache),
                    allow_grid_cache,
                );
            });
        });
    }

    #[test]
    fn selection_render_invalidates_grid_cache_instead_of_storing_highlight() {
        let ctx = egui::Context::default();
        let mut term = test_term_with_text("hello world");
        let metrics = GridMetrics {
            char_width: 8.0,
            line_height: 16.0,
            font_id: FontId::monospace(13.0),
        };
        let mut cache = TerminalGridCache::default();

        render_grid_pass(&ctx, &mut term, &metrics, &mut cache, true);
        let clean_key = cache.key;
        let clean_shape_count = cache.shapes.len();
        assert!(clean_key.is_some());
        assert!(clean_shape_count > 0);

        let mut selection = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left);
        selection.update(Point::new(Line(0), Column(4)), Side::Right);
        term.selection = Some(selection);
        render_grid_pass(&ctx, &mut term, &metrics, &mut cache, false);
        // The selection is cleared outside the render pass by copy/cut, so a
        // cache primed here would replay the highlight on the next quiet frame.
        assert_eq!(cache.key, None);
        assert!(cache.shapes.is_empty());

        term.selection = None;
        render_grid_pass(&ctx, &mut term, &metrics, &mut cache, true);
        assert_eq!(cache.key, clean_key);
        assert_eq!(cache.shapes.len(), clean_shape_count);
    }

    #[test]
    fn merge_shape_layers_keeps_layer_order() {
        let background_a = Shape::rect_filled(
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(8.0, 16.0)),
            0,
            Color32::RED,
        );
        let background_b = Shape::rect_filled(
            Rect::from_min_max(Pos2::new(8.0, 0.0), Pos2::new(16.0, 16.0)),
            0,
            Color32::BLUE,
        );
        let foreground_a = Shape::circle_filled(Pos2::new(4.0, 8.0), 2.0, Color32::WHITE);
        let foreground_b = Shape::line_segment([Pos2::new(0.0, 15.0), Pos2::new(8.0, 15.0)], (1.0, Color32::WHITE));

        let merged = merge_shape_layers(
            vec![background_a.clone(), background_b.clone()],
            vec![foreground_a.clone(), foreground_b.clone()],
        );

        assert_eq!(merged, vec![background_a, background_b, foreground_a, foreground_b]);
    }

    #[test]
    fn block_cursor_only_hides_selection_at_actual_cursor_position() {
        let cell = Cell::default();
        let indexed = Indexed {
            point: Point::new(Line(0), Column(2)),
            cell: &cell,
        };
        let selection = SelectionRange::new(indexed.point, Point::new(Line(0), Column(4)), false);

        assert!(selection.contains_cell(&indexed, Point::new(Line(0), Column(7)), CursorShape::Block));
        assert!(!selection.contains_cell(&indexed, indexed.point, CursorShape::Block));
    }

    #[test]
    fn dim_foreground_is_flattened_before_contrast_in_light_theme() {
        theme::set_theme(theme::ResolvedTheme::Light);

        let cell = Cell {
            fg: TerminalColor::Named(NamedColor::DimForeground),
            bg: TerminalColor::Named(NamedColor::Background),
            ..Cell::default()
        };

        let (fg, bg) = cell_colors(&cell, false, &Colors::default());
        let expected = theme::ensure_terminal_text_contrast(
            theme::composite_over(theme::PANEL_BG(), theme::alpha(theme::FG_SOFT(), 196)),
            theme::PANEL_BG(),
        );

        assert_eq!(bg, theme::PANEL_BG());
        assert_eq!(fg, expected);
        assert_eq!(fg.a(), u8::MAX);
    }
}
