use std::ops::Range;

use alacritty_terminal::grid::{GridIterator, Indexed};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::point_to_viewport;
use egui::epaint::text::FontsView;
use egui::{CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use horizon_core::next_terminal_file_citation;

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
    while let Some((byte_range, citation)) = next_terminal_file_citation(&text, search_start) {
        let start_char = text[..byte_range.start].chars().count();
        let char_count = text[byte_range.clone()].chars().count();
        let char_range = start_char..start_char + char_count;
        let selected = selection.is_some_and(|selection| {
            citation_cell_indices(char_range.clone(), &char_cells).any(|index| selection.contains(cells[index].point))
        });
        let segments = (!selected)
            .then(|| citation_segments(char_range.clone(), &char_cells, cells, display_offset, rect, metrics));
        if let Some(placement) = segments
            .as_deref()
            .and_then(|items| items.iter().max_by(|a, b| a.width().total_cmp(&b.width())))
            && append_chip(fonts, &mut shapes, *placement, &citation.display_label, metrics)
        {
            for cell_index in citation_cell_indices(char_range, &char_cells) {
                hidden_cells[cell_index] = true;
            }
        }
        search_start = byte_range.end;
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
        text.push(if indexed.cell.zerowidth().is_some() {
            '\u{FFFD}'
        } else {
            indexed.cell.c
        });
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
    display_offset: usize,
    rect: Rect,
    metrics: &GridMetrics,
) -> Vec<Rect> {
    let mut segments: Vec<Rect> = Vec::new();
    let mut previous_point = None;
    for cell_index in citation_cell_indices(chars, char_cells) {
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

fn citation_cell_indices(chars: Range<usize>, char_cells: &[Option<usize>]) -> impl Iterator<Item = usize> + '_ {
    char_cells.get(chars).into_iter().flatten().flatten().copied()
}

fn append_chip(
    fonts: &mut FontsView<'_>,
    shapes: &mut Vec<Shape>,
    placement: Rect,
    display_label: &str,
    metrics: &GridMetrics,
) -> bool {
    let color = theme::PALETTE_CYAN();
    let Some((pad_x, available_chars)) = chip_text_layout(placement.width(), metrics.char_width) else {
        return false;
    };
    let Some(label) = fitted_label(display_label, available_chars) else {
        return false;
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
    true
}

fn chip_text_layout(placement_width: f32, char_width: f32) -> Option<(f32, usize)> {
    let inner_width = (placement_width - 2.0).max(0.0);
    if inner_width < char_width {
        return None;
    }
    let pad_x = ((inner_width - char_width) / 2.0).clamp(0.0, 5.0);
    let available_chars = f32_to_usize(((inner_width - pad_x * 2.0) / char_width).floor());
    Some((pad_x, available_chars))
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

#[cfg(test)]
mod tests {
    use alacritty_terminal::grid::Indexed;
    use alacritty_terminal::index::{Column, Line, Point};
    use alacritty_terminal::selection::SelectionRange;
    use alacritty_terminal::term::cell::{Cell, Flags};
    use horizon_core::next_terminal_file_citation;

    use super::{chip_text_layout, citation_cell_indices, visible_text};
    #[test]
    fn chip_layout_reduces_padding_for_two_columns_and_rejects_one() {
        let (padding, available_chars) = chip_text_layout(16.0, 8.0).expect("two-column chip");
        assert!(padding < 5.0);
        assert_eq!(available_chars, 1);
        assert!(chip_text_layout(8.0, 8.0).is_none());
    }
    #[test]
    fn wrapped_citation_mapping_covers_hidden_and_selected_cells() {
        let token = r#":codex-file-citation{path="/tmp/report.pdf" purpose="source"}"#;
        let split = 40;
        let mut raw: Vec<_> = token.chars().map(|c| Cell { c, ..Cell::default() }).collect();
        raw[split - 1].flags.insert(Flags::WRAPLINE);
        let cells: Vec<_> = raw
            .iter()
            .enumerate()
            .map(|(index, cell)| Indexed {
                point: Point::new(Line(i32::from(index >= split)), Column(index % split)),
                cell,
            })
            .collect();
        let (text, map) = visible_text(&cells);
        let (bytes, _) = next_terminal_file_citation(&text, 0).expect("citation");
        let chars = text[..bytes.start].chars().count()..text[..bytes.end].chars().count();
        let mapped: Vec<_> = citation_cell_indices(chars, &map).collect();
        let mut hidden = vec![false; raw.len()];
        for &index in &mapped {
            hidden[index] = true;
        }
        let selection = SelectionRange::new(cells[split].point, cells[split].point, false);
        assert_eq!(text, token);
        assert_eq!(mapped.len(), raw.len());
        assert_eq!(hidden, vec![true; raw.len()]);
        assert!(mapped.iter().any(|&index| selection.contains(cells[index].point)));
    }
    #[test]
    fn malformed_and_unicode_tokens_have_no_display_match() {
        let cases = [
            r#":codex-file-citation{path="/tmp/a"purpose="source"}"#,
            r#":codex-file-citation{path="/tmp/résumé.pdf" purpose="source"}"#,
        ];
        assert!(cases.iter().all(|text| next_terminal_file_citation(text, 0).is_none()));
    }
}
