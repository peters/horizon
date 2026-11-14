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

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{BoardColumn, BoardItem, BoardRow, BoardState};

use crate::theme;
use crate::typeface::Weight;

use super::form::Form;
use super::metrics::{
    BOARD_CELL_GAP, BOARD_CELL_PAD, BOARD_COLUMN_FONT, BOARD_CURRENT_LINE, BOARD_CURRENT_TINT, BOARD_GROUP_GAP,
    BOARD_GROUP_HEAD_GAP, BOARD_GROUP_VALUE_FONT, BOARD_HAIR_TINT, BOARD_HEAD_COLUMN, BOARD_HEAD_GAP,
    BOARD_HEAD_HEIGHT, BOARD_HEAD_PAD_SIDE, BOARD_NOTE_FONT, BOARD_RADIUS, BOARD_ROW_GAP, BOARD_ROW_LABEL_FONT,
    BOARD_ROW_MIN_HEIGHT, BOARD_ROW_SUBTITLE_FONT, BOARD_TAG_FONT, BOARD_TAG_RADIUS, BOARD_TAG_SIZE, BOARD_VALUE_FONT,
    CAPTION_FONT, CAPTION_TRACKING, ITEM_BORDER, ITEM_DONE_NOTE_TINT, ITEM_HALO, ITEM_HALO_ALPHA, ITEM_LINE_GAP,
    ITEM_MARK, ITEM_MARK_GAP, ITEM_MARK_RING, ITEM_NOTE_FONT, ITEM_NOTE_LINE_HEIGHT, ITEM_PAD_SIDE, ITEM_PAD_Y,
    ITEM_RADIUS, ITEM_TAG_FONT, ITEM_TAG_GAP, ITEM_TAG_PAD, ITEM_TAG_RADIUS, ITEM_TITLE_FONT, ITEM_VALUE_FONT,
    ITEM_VALUE_GAP, LOG_TEXT_FONT, LOG_TIME_FONT, count,
};
use super::text;

/// How much of the accent or the warn colour the fill of an item carries.
const TINT_ALPHA: u8 = 20;
/// How much of the accent or the warn colour the outline of an item carries.
const OUTLINE_ALPHA: u8 = 175;
/// Width of the mark of a state, as a stroke.
const MARK_STROKE: f32 = 2.0;

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
    match form {
        Form::Wide => show_grid(ui, index, columns, rows, items),
        Form::Narrow => show_list(ui, index, columns, rows, items),
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

fn show_grid(ui: &mut Ui, index: usize, columns: &[BoardColumn], rows: &[BoardRow], items: &[BoardItem]) {
    let width = ui.available_width();
    let column_width = ((width - BOARD_HEAD_COLUMN) / count(columns.len())).max(1.0);
    let row_height = (0..rows.len())
        .map(|row| cell_height(tallest_cell(items, columns.len(), row)))
        .fold(BOARD_ROW_MIN_HEIGHT, f32::max);
    let height = BOARD_HEAD_HEIGHT + count(rows.len()) * row_height;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter().clone();
    let term = theme::BG_ELEVATED();
    let radius = CornerRadius::same(BOARD_RADIUS);
    painter.rect_filled(rect, radius, term);
    let head = Rect::from_min_size(rect.min, Vec2::new(width, BOARD_HEAD_HEIGHT));
    let top_corners = CornerRadius {
        nw: BOARD_RADIUS,
        ne: BOARD_RADIUS,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(head, top_corners, theme::PANEL_BG());
    let lane = Rect::from_min_size(rect.min, Vec2::new(BOARD_HEAD_COLUMN, height));
    let left_corners = CornerRadius {
        nw: BOARD_RADIUS,
        ne: 0,
        sw: BOARD_RADIUS,
        se: 0,
    };
    painter.rect_filled(lane, left_corners, theme::PANEL_BG());
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
        painter.rect_filled(square, CornerRadius::same(BOARD_TAG_RADIUS), fill);
        let galley = text::line_weight(
            painter,
            tag,
            &FontId::monospace(BOARD_TAG_FONT),
            ink,
            BOARD_TAG_SIZE,
            Weight::Medium,
        );
        painter.galley(
            Pos2::new(
                square.center().x - galley.size().x * 0.5,
                square.center().y - galley.size().y * 0.5,
            ),
            galley,
            ink,
        );
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
        let note = text::wrapped_weight(
            painter,
            item.note.as_deref().unwrap_or_default(),
            &FontId::monospace(ITEM_NOTE_FONT),
            colours(item.state).note,
            note_room(rect),
            Some(ITEM_NOTE_LINE_HEIGHT),
            Weight::Regular,
        );
        paint_chip(painter, rect, &chip, &note);
        chip_node(ui, index, item_index, rect, &chip);
        top += chip_height(1) + BOARD_CELL_GAP;
    }
}

/// The room that the evidence of one item takes: the width of the item without
/// its padding, its mark and the gap to its title.
fn note_room(rect: Rect) -> f32 {
    (rect.width() - 2.0 * ITEM_BORDER - 2.0 * ITEM_PAD_SIDE - ITEM_MARK - ITEM_MARK_GAP).max(0.0)
}

/// One item of a board, as the parts of its two lines.
struct Chip<'a> {
    title: &'a str,
    value: Option<&'a str>,
    tag: Option<&'a str>,
    note: &'a str,
    state: BoardState,
    place: Option<&'a str>,
}

impl Chip<'_> {
    /// The whole text of the item, for the tooltip of a text that was cut.
    fn whole(&self) -> String {
        let mut text = String::from(self.title);
        if let Some(tag) = self.tag {
            text.push_str(" · ");
            text.push_str(tag);
        }
        if let Some(value) = self.value {
            text.push_str("  ");
            text.push_str(value);
        }
        if !self.note.is_empty() {
            text.push('\n');
            if let Some(place) = self.place {
                text.push_str(place);
                text.push_str(" · ");
            }
            text.push_str(self.note);
        }
        text
    }
}

/// The colours of one item, from the state that it states.
struct ChipColours {
    fill: Color32,
    outline: Color32,
    title: Color32,
    note: Color32,
    value: Color32,
}

fn colours(state: BoardState) -> ChipColours {
    let (tint, fill) = match state {
        BoardState::Now => (theme::ACCENT(), theme::alpha(theme::ACCENT(), TINT_ALPHA)),
        BoardState::NeedsYou => (
            theme::PALETTE_YELLOW(),
            theme::alpha(theme::PALETTE_YELLOW(), TINT_ALPHA),
        ),
        _ => (theme::ACCENT(), theme::PANEL_BG()),
    };
    let outline = match state {
        BoardState::Now | BoardState::NeedsYou => theme::alpha(tint, OUTLINE_ALPHA),
        BoardState::Failed => theme::PALETTE_RED(),
        _ => theme::BORDER_SUBTLE(),
    };
    let value = match state {
        BoardState::NeedsYou => theme::PALETTE_YELLOW(),
        _ => theme::FG_SOFT(),
    };
    let note = match state {
        BoardState::Done => theme::blend(theme::FG_DIM(), theme::PALETTE_GREEN(), ITEM_DONE_NOTE_TINT),
        _ => theme::FG_DIM(),
    };
    let title = if state == BoardState::Waiting {
        theme::FG_SOFT()
    } else {
        theme::FG()
    };
    ChipColours {
        fill,
        outline,
        title,
        note,
        value,
    }
}

/// Paint one item: its fill, its outline, the mark of its state, its title with
/// its tag, its value at the right edge and its evidence under them.
fn paint_chip(painter: &egui::Painter, rect: Rect, chip: &Chip<'_>, note: &std::sync::Arc<egui::Galley>) {
    let colours = colours(chip.state);
    let radius = CornerRadius::same(ITEM_RADIUS);
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(ITEM_BORDER, colours.outline),
        StrokeKind::Inside,
    );
    let top = rect.min.y + ITEM_BORDER + ITEM_PAD_Y;
    let left = rect.min.x + ITEM_BORDER + ITEM_PAD_SIDE;
    let right = rect.max.x - ITEM_BORDER - ITEM_PAD_SIDE;
    paint_mark(
        painter,
        Pos2::new(left + ITEM_MARK * 0.5, top + ITEM_MARK * 0.5),
        chip.state,
    );
    let title_left = left + ITEM_MARK + ITEM_MARK_GAP;
    let value = chip.value.map(|value| {
        text::line(
            painter,
            value,
            &FontId::monospace(ITEM_VALUE_FONT),
            colours.value,
            f32::INFINITY,
        )
    });
    let value_width = value.as_ref().map_or(0.0, |galley| galley.size().x);
    let edge = if value.is_some() {
        ITEM_VALUE_GAP + value_width
    } else {
        0.0
    };
    let tag_width = chip.tag.map_or(0.0, |tag| {
        text::line_weight(
            painter,
            tag,
            &FontId::monospace(ITEM_TAG_FONT),
            colours.value,
            f32::INFINITY,
            Weight::Medium,
        )
        .size()
        .x + 2.0 * ITEM_TAG_PAD
            + ITEM_TAG_GAP
    });
    let title_room = (right - title_left - edge - tag_width).max(0.0);
    let title = text::line_weight(
        painter,
        chip.title,
        &FontId::proportional(ITEM_TITLE_FONT),
        colours.title,
        title_room,
        Weight::Semibold,
    );
    let title_y = top + (ITEM_MARK - title.size().y) * 0.5;
    painter.galley(Pos2::new(title_left, title_y), title.clone(), colours.title);
    if let Some(tag) = chip.tag {
        paint_tag(painter, Pos2::new(title_left + title.size().x + ITEM_TAG_GAP, top), tag);
    }
    if let Some(galley) = value {
        let height = galley.size().y;
        painter.galley(
            Pos2::new(right - galley.size().x, top + (ITEM_MARK - height) * 0.5),
            galley,
            colours.value,
        );
    }
    painter.galley(
        Pos2::new(title_left, top + ITEM_MARK + ITEM_LINE_GAP),
        note.clone(),
        colours.note,
    );
}

/// Paint the small outlined word after the title of an item.
fn paint_tag(painter: &egui::Painter, at: Pos2, tag: &str) {
    let galley = text::line_weight(
        painter,
        tag,
        &FontId::monospace(ITEM_TAG_FONT),
        theme::ACCENT(),
        f32::INFINITY,
        Weight::Medium,
    );
    let rect = Rect::from_min_size(
        Pos2::new(at.x, at.y + (ITEM_MARK - galley.size().y) * 0.5 - ITEM_TAG_GAP * 0.5),
        Vec2::new(galley.size().x + 2.0 * ITEM_TAG_PAD, galley.size().y + ITEM_TAG_GAP),
    );
    let radius = CornerRadius::same(ITEM_TAG_RADIUS);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::alpha(theme::ACCENT(), 115)),
        StrokeKind::Inside,
    );
    painter.galley(
        Pos2::new(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        theme::ACCENT(),
    );
}

/// Paint the mark of one state: never a ring that a person could read as a
/// choice of theirs.
fn paint_mark(painter: &egui::Painter, centre: Pos2, state: BoardState) {
    let radius = ITEM_MARK * 0.5;
    let ring = ITEM_MARK_RING;
    match state {
        BoardState::Done => {
            painter.circle_filled(centre, radius, theme::PALETTE_GREEN());
            ink(painter, centre, radius, true);
        }
        BoardState::Failed => {
            painter.circle_filled(centre, radius, theme::PALETTE_RED());
            ink(painter, centre, radius, false);
        }
        BoardState::Now | BoardState::NeedsYou => {
            let colour = if state == BoardState::Now {
                theme::ACCENT()
            } else {
                theme::PALETTE_YELLOW()
            };
            painter.circle_filled(centre, radius + ITEM_HALO, theme::alpha(colour, ITEM_HALO_ALPHA));
            painter.circle_stroke(centre, radius - ring * 0.5, Stroke::new(ring, colour));
            painter.circle_filled(centre, radius * 0.55, colour);
        }
        BoardState::Waiting => {
            painter.circle_stroke(centre, radius - ring * 0.5, Stroke::new(ring, theme::FG_DIM()));
        }
    }
}

/// The dark check of a finished item, or the dark cross of a failed one.
fn ink(painter: &egui::Painter, centre: Pos2, radius: f32, check: bool) {
    let stroke = Stroke::new(MARK_STROKE, theme::BG());
    let (a, b, c) = if check {
        (
            Pos2::new(centre.x - radius * 0.45, centre.y),
            Pos2::new(centre.x - radius * 0.1, centre.y + radius * 0.35),
            Pos2::new(centre.x + radius * 0.5, centre.y - radius * 0.35),
        )
    } else {
        (
            Pos2::new(centre.x - radius * 0.4, centre.y - radius * 0.4),
            Pos2::new(centre.x + radius * 0.4, centre.y + radius * 0.4),
            Pos2::new(centre.x - radius * 0.4, centre.y + radius * 0.4),
        )
    };
    painter.line_segment([a, b], stroke);
    painter.line_segment([b, c], stroke);
    if !check {
        painter.line_segment([a, c], stroke);
    }
}

/// Registers one item, so a reader hears its title and a resting pointer shows
/// the whole text of a text that the edge of the card cut.
fn chip_node(ui: &Ui, index: usize, at: usize, rect: Rect, chip: &Chip<'_>) {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_board_item", index, at)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, chip.title));
    if response.hovered() {
        response.on_hover_text(chip.whole());
    }
}

/// The narrow form: the same items as one list, grouped by wave.
fn show_list(ui: &mut Ui, index: usize, columns: &[BoardColumn], rows: &[BoardRow], items: &[BoardItem]) {
    let fold = foldable(columns, items);
    let folded = !fold.is_empty() && ui.data(|data| data.get_temp::<bool>(fold_id(ui, index)).unwrap_or(true));
    let mut first = true;
    if !fold.is_empty() {
        let done = fold.iter().map(|column| row_count(items, *column)).sum::<usize>();
        show_fold(ui, index, done, folded);
        first = false;
    }
    for (column, wave) in columns.iter().enumerate() {
        if folded && fold.contains(&column) {
            continue;
        }
        if row_count(items, column) == 0 {
            continue;
        }
        if !first {
            ui.add_space(BOARD_GROUP_GAP);
        }
        first = false;
        show_group_head(ui, wave);
        for (item_index, item) in items.iter().enumerate().filter(|(_, item)| item.column == column) {
            ui.add_space(BOARD_GROUP_HEAD_GAP);
            show_list_item(ui, index, item_index, item, rows);
        }
    }
}

fn row_count(items: &[BoardItem], column: usize) -> usize {
    items.iter().filter(|item| item.column == column).count()
}

fn fold_id(ui: &Ui, index: usize) -> egui::Id {
    ui.make_persistent_id(("block_board_fold", index))
}

/// The leading waves whose items all finished. The narrow form folds them into
/// one line, so a finished part of the plan keeps no room on the card.
fn foldable(columns: &[BoardColumn], items: &[BoardItem]) -> Vec<usize> {
    let mut fold = Vec::new();
    for (column, _) in columns.iter().enumerate() {
        let group = items.iter().filter(|item| item.column == column);
        if group.clone().next().is_none() || !group.into_iter().all(|item| item.state == BoardState::Done) {
            break;
        }
        fold.push(column);
    }
    fold
}

/// The caption of one group: its tag, its name and the word `now` when the wave
/// runs, with the value of the wave at the right edge.
fn show_group_head(ui: &mut Ui, wave: &BoardColumn) {
    let width = ui.available_width();
    let value = wave.value.as_ref().map(|value| {
        text::line(
            ui.painter(),
            value,
            &FontId::monospace(BOARD_GROUP_VALUE_FONT),
            theme::FG_DIM(),
            f32::INFINITY,
        )
    });
    let value_width = value.as_ref().map_or(0.0, |galley| galley.size().x);
    let mut words = wave.label.clone();
    if let Some(tag) = &wave.tag {
        words = format!("{tag} · {words}");
    }
    if wave.current {
        words.push_str(" · now");
    }
    let colour = if wave.current { theme::ACCENT() } else { theme::FG_DIM() };
    let caption = text::spaced_line(
        ui.painter(),
        &words,
        &FontId::proportional(CAPTION_FONT),
        colour,
        (width - value_width - BOARD_HEAD_GAP).max(0.0),
        CAPTION_TRACKING,
        Weight::Semibold,
    );
    let height = caption
        .size()
        .y
        .max(value.as_ref().map_or(0.0, |galley| galley.size().y));
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, &words));
    ui.painter().galley(rect.min, caption, colour);
    if let Some(galley) = value {
        let size = galley.size();
        ui.painter().galley(
            Pos2::new(rect.max.x - size.x, rect.center().y - size.y * 0.5),
            galley,
            theme::FG_DIM(),
        );
    }
}

/// One item of the narrow list: the whole width of the card, with the place of
/// the item in front of its evidence.
fn show_list_item(ui: &mut Ui, index: usize, at: usize, item: &BoardItem, rows: &[BoardRow]) {
    let width = ui.available_width();
    let place = rows.get(item.row).map(|worker| worker.label.as_str());
    let mut note_text = String::from(place.unwrap_or_default());
    if let Some(note) = item.note.as_deref().filter(|note| !note.is_empty()) {
        note_text.push_str(" · ");
        note_text.push_str(note);
    }
    let chip = Chip {
        title: &item.title,
        value: item.value.as_deref(),
        tag: item.tag.as_deref(),
        note: &note_text,
        state: item.state,
        place,
    };
    let room = (width - 2.0 * ITEM_BORDER - 2.0 * ITEM_PAD_SIDE - ITEM_MARK - ITEM_MARK_GAP).max(0.0);
    let note = text::wrapped_weight(
        ui.painter(),
        &note_text,
        &FontId::monospace(ITEM_NOTE_FONT),
        colours(item.state).note,
        room,
        Some(ITEM_NOTE_LINE_HEIGHT),
        Weight::Regular,
    );
    let height = chip_height(1) + note.size().y - ITEM_NOTE_LINE_HEIGHT;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    paint_chip(ui.painter(), rect, &chip, &note);
    chip_node(ui, index, at, rect, &chip);
}

/// The line that folds the finished waves: the mark of a finished item, how
/// many finished and the word that shows them again. A press changes only the
/// view of this card.
fn show_fold(ui: &mut Ui, index: usize, done: usize, folded: bool) {
    let width = ui.available_width();
    let height = ITEM_MARK + 2.0 * BOARD_CELL_PAD;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let label = format!("{done} done");
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
    if response.clicked() {
        ui.data_mut(|data| data.insert_temp(fold_id(ui, index), !folded));
        ui.ctx().request_repaint();
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::same(ITEM_RADIUS),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    paint_mark(
        painter,
        Pos2::new(rect.min.x + ITEM_PAD_SIDE + ITEM_MARK * 0.5, rect.center().y),
        BoardState::Done,
    );
    let count = text::line(
        painter,
        &label,
        &FontId::proportional(LOG_TEXT_FONT),
        theme::FG_SOFT(),
        f32::INFINITY,
    );
    painter.galley(
        Pos2::new(
            rect.min.x + ITEM_PAD_SIDE + ITEM_MARK + ITEM_MARK_GAP,
            rect.center().y - count.size().y * 0.5,
        ),
        count,
        theme::FG_SOFT(),
    );
    let word = text::line(
        painter,
        if folded { "show" } else { "hide" },
        &FontId::monospace(LOG_TIME_FONT),
        theme::FG_DIM(),
        f32::INFINITY,
    );
    painter.galley(
        Pos2::new(
            rect.max.x - ITEM_PAD_SIDE - word.size().x,
            rect.center().y - word.size().y * 0.5,
        ),
        word,
        theme::FG_DIM(),
    );
}
