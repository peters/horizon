//! The board: who and where the work runs, against the waves that run it.
//!
//! The wide card draws the grid of the approved board mockup: one column for
//! each wave, one row for each worker and the items of the work in the cells,
//! each item with the one mark of its state. The narrow card draws the same
//! items as one list, grouped by wave, with the place of every item on its
//! second line and the finished leading waves folded into one line.
//!
//! An item takes no press and reaches no agent. The fold of the narrow form is
//! the person's own view of the card: it is kept for that card and it is no
//! click of a `next_click`.

mod chip;
mod grid;
mod list;

/// The lines of the card that the evidence of one item of the list may take.
pub(super) const NOTE_LINES: usize = 2;

use egui::Ui;
use horizon_browser_control::block::{BoardColumn, BoardItem, BoardRow};

use super::form::Form;
use super::metrics::{
    BOARD_CELL_GAP, BOARD_CELL_PAD, BOARD_COLUMN_MIN_WIDTH, BOARD_HEAD_COLUMN, ITEM_BORDER, ITEM_LINE_GAP, ITEM_MARK,
    ITEM_NOTE_LINE_HEIGHT, ITEM_PAD_Y, count,
};

/// The least body width that draws the grid of `columns` waves: the column of
/// the workers and one column of items for every wave.
pub(super) fn grid_min_width(columns: usize) -> f32 {
    BOARD_HEAD_COLUMN + count(columns) * BOARD_COLUMN_MIN_WIDTH
}

/// Draw a `board` component in the form that the body is wide enough for.
pub(super) fn show(
    ui: &mut Ui,
    index: usize,
    columns: &[BoardColumn],
    rows: &[BoardRow],
    items: &[BoardItem],
    form: Form,
) {
    if columns.is_empty() || rows.is_empty() {
        return;
    }
    // The grid needs its room: a wave whose column would be narrower than
    // BOARD_COLUMN_MIN_WIDTH is drawn as the list of the narrow form instead,
    // also inside a card that is otherwise in the wide form.
    let form = if form.is_wide() && ui.available_width() >= grid_min_width(columns.len()) {
        Form::Wide
    } else {
        Form::Narrow
    };
    match form {
        Form::Wide => grid::show(ui, index, columns, rows, items),
        Form::Narrow => list::show(ui, index, columns, rows, items),
    }
}

/// The height of one item whose evidence takes `note_lines` lines.
pub(super) fn chip_height(note_lines: usize) -> f32 {
    2.0 * ITEM_BORDER + 2.0 * ITEM_PAD_Y + ITEM_MARK + ITEM_LINE_GAP + count(note_lines) * ITEM_NOTE_LINE_HEIGHT
}

/// The height of the cell that holds `items` items, and of the body row that
/// holds that cell.
pub(super) fn cell_height(items: usize) -> f32 {
    if items == 0 {
        0.0
    } else {
        2.0 * BOARD_CELL_PAD + count(items) * chip_height(1) + count(items - 1) * BOARD_CELL_GAP
    }
}
