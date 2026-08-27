use std::ops::Range;

use alacritty_terminal::grid::{GridIterator, Indexed};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::point_to_viewport;
use egui::epaint::text::FontsView;
use egui::{CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use horizon_core::{TERMINAL_FILE_CITATION_PREFIX, parse_terminal_file_citation, terminal_file_citation_end};

use crate::theme;

use super::layout::{GridMetrics, f32_to_usize, usize_to_f32};

pub(super) enum CitationDisplayCells<'a> {
    Direct(GridIterator<'a, Cell>),
    Buffered(std::vec::IntoIter<Indexed<&'a Cell>>),
}

impl<'a> CitationDisplayCells<'a> {
    pub(super) fn new(cells: GridIterator<'a, Cell>, enabled: bool) -> Self {
        if enabled {
            Self::Buffered(cells.collect::<Vec<_>>().into_iter())
        } else {
            Self::Direct(cells)
        }
    }

    pub(super) fn citation_cells(&self) -> Option<&[Indexed<&'a Cell>]> {
        match self {
            Self::Direct(_) => None,
            Self::Buffered(cells) => Some(cells.as_slice()),
        }
    }
}

impl<'a> Iterator for CitationDisplayCells<'a> {
    type Item = Indexed<&'a Cell>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Direct(cells) => cells.next(),
            Self::Buffered(cells) => cells.next(),
        }
    }
}

pub(super) fn file_citation_shapes(
    fonts: &mut FontsView<'_>,
    hidden_cells: &mut [bool],
    cells: &[Indexed<&Cell>],
    selection: Option<SelectionRange>,
    display_offset: usize,
    rect: Rect,
    metrics: &GridMetrics,
) -> Vec<Shape> {
    let mut shapes = Vec::new();
    if !cells.iter().any(|indexed| indexed.cell.c == ':') {
        return shapes;
    }
    let (text, char_cells) = visible_text(cells);
    let mut search_start = 0;
    while let Some(relative_start) = text[search_start..].find(TERMINAL_FILE_CITATION_PREFIX) {
        let start = search_start + relative_start;
        let Some(end) = terminal_file_citation_end(&text, start) else {
            break;
        };
        if let Some(citation) = parse_terminal_file_citation(&text[start..end]) {
            let start_char = text[..start].chars().count();
            let char_count = text[start..end].chars().count();
            let char_range = start_char..start_char + char_count;
            let selected = selection.is_some_and(|selection| {
                char_cells[char_range.clone()]
                    .iter()
                    .flatten()
                    .any(|&index| selection.contains(cells[index].point))
            });
            let segments = (!selected).then(|| {
                citation_segments(
                    char_range,
                    &char_cells,
                    cells,
                    hidden_cells,
                    display_offset,
                    rect,
                    metrics,
                )
            });
            if let Some(placement) = segments
                .as_deref()
                .and_then(|items| items.iter().max_by(|a, b| a.width().total_cmp(&b.width())))
            {
                append_chip(fonts, &mut shapes, *placement, &citation.display_label, metrics);
            }
        }
        search_start = end;
    }
    shapes
}

fn visible_text(cells: &[Indexed<&Cell>]) -> (String, Vec<Option<usize>>) {
    let mut text = String::with_capacity(cells.len());
    let mut char_cells = Vec::with_capacity(cells.len());
    let mut previous_line = None;
    let mut previous_row_wrapped = false;
    for (index, indexed) in cells.iter().enumerate() {
        if previous_line.is_some_and(|line| line != indexed.point.line) && !previous_row_wrapped {
            text.push('\n');
            char_cells.push(None);
        }
        text.push(indexed.cell.c);
        char_cells.push(Some(index));
        previous_line = Some(indexed.point.line);
        previous_row_wrapped = indexed.cell.flags.contains(Flags::WRAPLINE);
    }
    (text, char_cells)
}

fn citation_segments(
    chars: Range<usize>,
    char_cells: &[Option<usize>],
    cells: &[Indexed<&Cell>],
    hidden_cells: &mut [bool],
    display_offset: usize,
    rect: Rect,
    metrics: &GridMetrics,
) -> Vec<Rect> {
    let mut segments: Vec<Rect> = Vec::new();
    let mut previous_point = None;
    for cell_index in char_cells.get(chars).into_iter().flatten().flatten().copied() {
        if let Some(hidden) = hidden_cells.get_mut(cell_index) {
            *hidden = true;
        }
        let Some(indexed) = cells.get(cell_index) else {
            continue;
        };
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let cell_rect = Rect::from_min_size(
            Pos2::new(
                rect.min.x + usize_to_f32(point.column.0) * metrics.char_width,
                rect.min.y + usize_to_f32(point.line) * metrics.line_height,
            ),
            Vec2::new(metrics.char_width, metrics.line_height),
        );
        if previous_point.is_some_and(|(line, column)| line == point.line && column + 1 == point.column.0) {
            if let Some(segment) = segments.last_mut() {
                segment.max.x = cell_rect.max.x;
            }
        } else {
            segments.push(cell_rect);
        }
        previous_point = Some((point.line, point.column.0));
    }
    segments
}

fn append_chip(
    fonts: &mut FontsView<'_>,
    shapes: &mut Vec<Shape>,
    placement: Rect,
    display_label: &str,
    metrics: &GridMetrics,
) {
    let color = theme::PALETTE_CYAN();
    let pad_x = 5.0;
    let available_chars = f32_to_usize(((placement.width() - pad_x * 2.0 - 2.0).max(0.0) / metrics.char_width).floor());
    let Some(label) = fitted_label(display_label, available_chars) else {
        return;
    };
    let desired_width = usize_to_f32(label.chars().count()) * metrics.char_width + pad_x * 2.0;
    let chip_rect = Rect::from_min_size(
        placement.min + Vec2::splat(1.0),
        Vec2::new(
            desired_width.min(placement.width() - 2.0),
            (placement.height() - 2.0).max(1.0),
        ),
    );
    shapes.push(Shape::rect_filled(
        chip_rect,
        CornerRadius::same(4),
        theme::blend(theme::PANEL_BG_ALT(), color, 0.16),
    ));
    shapes.push(Shape::rect_stroke(
        chip_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::alpha(color, 150)),
        StrokeKind::Inside,
    ));
    shapes.push(Shape::text(
        fonts,
        Pos2::new(chip_rect.min.x + pad_x, placement.min.y),
        egui::Align2::LEFT_TOP,
        label,
        metrics.font_id.clone(),
        color,
    ));
}

fn fitted_label(label: &str, available_chars: usize) -> Option<String> {
    if available_chars == 0 {
        return None;
    }
    let count = label.chars().count();
    if count <= available_chars {
        return Some(label.to_owned());
    }
    if available_chars == 1 {
        return Some("…".to_owned());
    }
    Some(label.chars().take(available_chars - 1).chain(['…']).collect())
}
