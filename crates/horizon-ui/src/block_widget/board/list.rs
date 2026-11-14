//! The list of the narrow form: one caption for each wave, the items of the
//! wave under it and one line that folds the finished waves.

use egui::{Color32, CornerRadius, FontId, Pos2, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::{BoardColumn, BoardItem, BoardRow, BoardState};

use super::chip::{Chip, chip_node, colours, paint_chip, paint_mark};
use super::chip_height;
use crate::theme;
use crate::typeface::Weight;

use super::super::metrics::{
    BOARD_CELL_PAD, BOARD_GROUP_GAP, BOARD_GROUP_HEAD_GAP, BOARD_GROUP_VALUE_FONT, BOARD_HEAD_GAP,
    BOARD_LIST_MAX_WIDTH, CAPTION_FONT, CAPTION_TRACKING, ITEM_BORDER, ITEM_MARK, ITEM_MARK_GAP, ITEM_NOTE_FONT,
    ITEM_NOTE_LINE_HEIGHT, ITEM_PAD_SIDE, LOG_TEXT_FONT, LOG_TIME_FONT, RADIUS_CONTROL,
};
use super::super::text;
use super::NOTE_LINES;
use crate::block_widget::form::Form;

/// The narrow form: the same items as one list, grouped by wave.
pub(super) fn show(ui: &mut Ui, index: usize, columns: &[BoardColumn], rows: &[BoardRow], items: &[BoardItem]) {
    // A card that draws the wide form keeps the list of its board at the left
    // edge, in a column that a reader follows with one look.
    let width = ui.available_width();
    if Form::of(width).is_wide() {
        ui.scope(|ui| {
            ui.set_max_width(width.min(BOARD_LIST_MAX_WIDTH));
            show_list(ui, index, columns, rows, items);
        });
        return;
    }
    show_list(ui, index, columns, rows, items);
}

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
        line_at(
            ui.painter(),
            rect.max.x - galley.size().x,
            rect.center().y,
            &galley,
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
    // The list gives the evidence two lines of the card, and no more.
    let note = text::wrapped_rows(
        ui.painter(),
        &note_text,
        &FontId::monospace(ITEM_NOTE_FONT),
        colours(item.state).note,
        room,
        Some(ITEM_NOTE_LINE_HEIGHT),
        NOTE_LINES,
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
        CornerRadius::same(RADIUS_CONTROL),
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

/// Paint `galley` with its left edge at `x`, centred on `middle`.
fn line_at(painter: &egui::Painter, x: f32, middle: f32, galley: &std::sync::Arc<egui::Galley>, colour: Color32) {
    painter.galley(Pos2::new(x, middle - galley.size().y * 0.5), galley.clone(), colour);
}
