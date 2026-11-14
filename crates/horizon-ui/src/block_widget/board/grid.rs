//! The grid of the wide form: the head row of waves, the worker column and one
//! cell of items for each wave and worker.

use egui::{CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_browser_control::block::{BoardColumn, BoardItem, BoardRow};

use super::cell_height;
use super::chip::{Chip, chip_node, colours, note_room, paint_chip};
use super::chip_height;
use crate::theme;
use crate::typeface::Weight;

use super::super::metrics::{
    BOARD_CELL_GAP, BOARD_CELL_PAD, BOARD_COLUMN_FONT, BOARD_COLUMN_MAX_WIDTH, BOARD_CURRENT_LINE, BOARD_CURRENT_TINT,
    BOARD_HAIR_TINT, BOARD_HEAD_COLUMN, BOARD_HEAD_GAP, BOARD_HEAD_HEIGHT, BOARD_HEAD_PAD_SIDE, BOARD_NOTE_FONT,
    BOARD_ROW_GAP, BOARD_ROW_LABEL_FONT, BOARD_ROW_MIN_HEIGHT, BOARD_ROW_SUBTITLE_FONT, BOARD_TAG_FONT, BOARD_TAG_SIZE,
    BOARD_VALUE_FONT, ITEM_NOTE_FONT, ITEM_NOTE_LINE_HEIGHT, RADIUS_FRAME, RADIUS_TAG, count,
};
use super::super::text;

pub(super) fn show(ui: &mut Ui, index: usize, columns: &[BoardColumn], rows: &[BoardRow], items: &[BoardItem]) {
    // A column of the grid is at most BOARD_COLUMN_MAX_WIDTH wide, so the grid
    // of a few waves keeps the left edge of the body instead of stretching its
    // cells across a card that the person made wide.
    let column_width =
        ((ui.available_width() - BOARD_HEAD_COLUMN) / count(columns.len())).clamp(1.0, BOARD_COLUMN_MAX_WIDTH);
    let width = BOARD_HEAD_COLUMN + count(columns.len()) * column_width;
    let row_height = (0..rows.len())
        .map(|row| cell_height(tallest_cell(items, columns.len(), row)))
        .fold(BOARD_ROW_MIN_HEIGHT, f32::max);
    let height = BOARD_HEAD_HEIGHT + count(rows.len()) * row_height;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let term = theme::BG_ELEVATED();
    let radius = CornerRadius::same(RADIUS_FRAME);
    painter.rect_filled(rect, radius, term);
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(width, BOARD_HEAD_HEIGHT)),
        CornerRadius {
            nw: RADIUS_FRAME,
            ne: RADIUS_FRAME,
            sw: 0,
            se: 0,
        },
        theme::PANEL_BG(),
    );
    painter.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(BOARD_HEAD_COLUMN, height)),
        CornerRadius {
            nw: RADIUS_FRAME,
            ne: 0,
            sw: RADIUS_FRAME,
            se: 0,
        },
        theme::PANEL_BG(),
    );
    for (column, wave) in columns.iter().enumerate() {
        if wave.current {
            let cell = Rect::from_min_size(
                Pos2::new(column_left(rect, column, column_width), rect.min.y + BOARD_HEAD_HEIGHT),
                Vec2::new(column_width, height - BOARD_HEAD_HEIGHT),
            );
            painter.rect_filled(
                cell,
                CornerRadius::ZERO,
                theme::blend(term, theme::ACCENT(), BOARD_CURRENT_TINT),
            );
        }
    }
    let stroke = Stroke::new(1.0, theme::blend(term, theme::BORDER_SUBTLE(), BOARD_HAIR_TINT));
    for column in 1..columns.len() {
        painter.vline(column_left(rect, column, column_width), rect.y_range(), stroke);
    }
    painter.hline(rect.x_range(), rect.min.y + BOARD_HEAD_HEIGHT, stroke);
    for row in 1..rows.len() {
        painter.hline(rect.x_range(), row_top(rect, row, row_height), stroke);
    }
    for (column, wave) in columns.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(column_left(rect, column, column_width), rect.min.y),
            Vec2::new(column_width, BOARD_HEAD_HEIGHT),
        );
        show_column_head(&painter, cell, wave);
        if wave.current {
            let y = cell.max.y - BOARD_CURRENT_LINE * 0.5;
            painter.hline(cell.x_range(), y, Stroke::new(BOARD_CURRENT_LINE, theme::ACCENT()));
        }
    }
    for (row, worker) in rows.iter().enumerate() {
        let top = row_top(rect, row, row_height);
        let label = Rect::from_min_size(Pos2::new(rect.min.x, top), Vec2::new(BOARD_HEAD_COLUMN, row_height));
        show_worker(&painter, label, worker);
        for column in 0..columns.len() {
            let cell = Cell {
                rect: Rect::from_min_size(
                    Pos2::new(column_left(rect, column, column_width), top),
                    Vec2::new(column_width, row_height),
                ),
                column,
                row,
            };
            show_cell(ui, &painter, index, cell, items, rows);
        }
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
}

fn column_left(rect: Rect, column: usize, column_width: f32) -> f32 {
    rect.min.x + BOARD_HEAD_COLUMN + count(column) * column_width
}

fn row_top(rect: Rect, row: usize, row_height: f32) -> f32 {
    rect.min.y + BOARD_HEAD_HEIGHT + count(row) * row_height
}

fn cell_items(items: &[BoardItem], column: usize, row: usize) -> Vec<(usize, &BoardItem)> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.column == column && item.row == row)
        .collect()
}

fn tallest_cell(items: &[BoardItem], columns: usize, row: usize) -> usize {
    (0..columns)
        .map(|column| cell_items(items, column, row).len())
        .max()
        .unwrap_or(0)
}

/// One cell of the grid: its rectangle and the column and row that it holds.
#[derive(Clone, Copy)]
struct Cell {
    rect: Rect,
    column: usize,
    row: usize,
}

fn show_cell(ui: &Ui, painter: &egui::Painter, index: usize, cell: Cell, items: &[BoardItem], rows: &[BoardRow]) {
    let Cell {
        rect: cell,
        column,
        row,
    } = cell;
    let at = cell_items(items, column, row);
    if at.is_empty() {
        return;
    }
    let used = count(at.len()) * chip_height(1) + count(at.len() - 1) * BOARD_CELL_GAP;
    let width = (cell.width() - 2.0 * BOARD_CELL_PAD).max(0.0);
    let mut top = cell.min.y + ((cell.height() - used) * 0.5).max(0.0);
    for (item_index, item) in at {
        let rect = Rect::from_min_size(
            Pos2::new(cell.min.x + BOARD_CELL_PAD, top),
            Vec2::new(width, chip_height(1)),
        );
        let chip = Chip {
            title: &item.title,
            value: item.value.as_deref(),
            tag: item.tag.as_deref(),
            note: item.note.as_deref().unwrap_or_default(),
            state: item.state,
            place: rows.get(item.row).map(|worker| worker.label.as_str()),
        };
        // The cell of the grid keeps one line of evidence: a longer note ends
        // with an ellipsis and the whole note shows while the pointer rests on
        // the item.
        let note = text::wrapped_rows(
            painter,
            item.note.as_deref().unwrap_or_default(),
            &FontId::monospace(ITEM_NOTE_FONT),
            colours(item.state).note,
            note_room(rect),
            text::Lines {
                line_height: Some(ITEM_NOTE_LINE_HEIGHT),
                max_rows: 1,
            },
            Weight::Regular,
        );
        paint_chip(painter, rect, &chip, &note);
        chip_node(ui, index, item_index, rect, &chip);
        top += chip_height(1) + BOARD_CELL_GAP;
    }
}

/// One head cell: the tag in a square, the name of the wave, its note and the
/// value at the right edge.
fn show_column_head(painter: &egui::Painter, cell: Rect, wave: &BoardColumn) {
    let mut left = cell.min.x + BOARD_HEAD_PAD_SIDE;
    let middle = cell.center().y;
    let value_width = wave.value.as_ref().map_or(0.0, |value| {
        let galley = text::line(
            painter,
            value,
            &FontId::monospace(BOARD_VALUE_FONT),
            theme::FG_DIM(),
            f32::INFINITY,
        );
        let width = galley.size().x;
        painter.galley(
            Pos2::new(cell.max.x - BOARD_HEAD_PAD_SIDE - width, middle - galley.size().y * 0.5),
            galley,
            theme::FG_DIM(),
        );
        width + BOARD_HEAD_GAP
    });
    if let Some(tag) = &wave.tag {
        let square = Rect::from_min_size(
            Pos2::new(left, middle - BOARD_TAG_SIZE * 0.5),
            Vec2::splat(BOARD_TAG_SIZE),
        );
        let (fill, ink) = if wave.current {
            (theme::ACCENT(), theme::BG())
        } else {
            (theme::PANEL_BG_ALT(), theme::FG_SOFT())
        };
        painter.rect_filled(square, CornerRadius::same(RADIUS_TAG), fill);
        let galley = text::line_weight(
            painter,
            tag,
            &FontId::monospace(BOARD_TAG_FONT),
            ink,
            BOARD_TAG_SIZE,
            Weight::Medium,
        );
        let offset = square.center() - galley.size() * 0.5;
        painter.galley(Pos2::new(offset.x, offset.y), galley, ink);
        left += BOARD_TAG_SIZE + BOARD_HEAD_GAP;
    }
    let room = (cell.max.x - BOARD_HEAD_PAD_SIDE - value_width - left).max(0.0);
    let label = text::line_weight(
        painter,
        &wave.label,
        &FontId::proportional(BOARD_COLUMN_FONT),
        theme::FG(),
        room,
        Weight::Semibold,
    );
    painter.galley(
        Pos2::new(left, middle - label.size().y * 0.5),
        label.clone(),
        theme::FG(),
    );
    if let Some(note) = &wave.note {
        let note_left = left + label.size().x + BOARD_HEAD_GAP;
        let note = text::line(
            painter,
            note,
            &FontId::monospace(BOARD_NOTE_FONT),
            theme::FG_DIM(),
            (cell.max.x - BOARD_HEAD_PAD_SIDE - value_width - note_left).max(0.0),
        );
        painter.galley(
            Pos2::new(note_left, middle - note.size().y * 0.5),
            note,
            theme::FG_DIM(),
        );
    }
}

/// One worker cell: who the work runs as and where it runs.
fn show_worker(painter: &egui::Painter, cell: Rect, worker: &BoardRow) {
    let room = (cell.width() - 2.0 * BOARD_HEAD_PAD_SIDE).max(0.0);
    let label = text::line_weight(
        painter,
        &worker.label,
        &FontId::proportional(BOARD_ROW_LABEL_FONT),
        theme::FG(),
        room,
        Weight::Semibold,
    );
    let subtitle = worker.subtitle.as_ref().map(|subtitle| {
        text::line(
            painter,
            subtitle,
            &FontId::proportional(BOARD_ROW_SUBTITLE_FONT),
            theme::FG_DIM(),
            room,
        )
    });
    let subtitle_height = subtitle.as_ref().map_or(0.0, |galley| galley.size().y);
    let span = label.size().y
        + if subtitle.is_some() {
            BOARD_ROW_GAP + subtitle_height
        } else {
            0.0
        };
    let mut y = cell.center().y - span * 0.5;
    painter.galley(
        Pos2::new(cell.min.x + BOARD_HEAD_PAD_SIDE, y),
        label.clone(),
        theme::FG(),
    );
    if let Some(galley) = subtitle {
        y += label.size().y + BOARD_ROW_GAP;
        painter.galley(Pos2::new(cell.min.x + BOARD_HEAD_PAD_SIDE, y), galley, theme::FG_DIM());
    }
}
