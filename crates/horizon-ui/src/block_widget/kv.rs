//! The label and value box: an inset surface with one pair per row.
//!
//! A pair is one label at the top of its row and one value beside it. A value
//! keeps the lines that its words need: a short value sits at the right edge of
//! its row, as it always did, and a value that needs more than one line starts at
//! the left edge of its own column, so its lines line up and nothing of it is
//! cut. The label takes the lines that it needs as well, in a column that keeps
//! at most [`LABEL_SHARE`] of the row, so a long value always has room.

use std::sync::Arc;

use egui::{CornerRadius, FontId, Galley, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::KvRow;

use crate::theme;

use super::metrics::{
    KV_COLUMN_GAP, KV_LABEL_FONT, KV_MONO_FONT, KV_PAD_SIDE, KV_RADIUS, KV_ROW_HEIGHT, KV_ROW_PAD_Y, KV_SEPARATOR,
    KV_SEPARATOR_ALPHA, KV_VALUE_FONT,
};
use super::text;

/// The largest share of a row that a label takes, so a long value never loses
/// half of its own column to its label.
const LABEL_SHARE: f32 = 0.4;

/// The two texts of one pair, laid out for the room that the row has.
struct Pair {
    label: Arc<Galley>,
    value: Arc<Galley>,
    /// The height of the row: the taller text, above and below its padding.
    height: f32,
}

/// Draw `rows` as one inset box.
pub(super) fn show(ui: &mut Ui, index: usize, rows: &[KvRow]) {
    if rows.is_empty() {
        return;
    }
    let width = ui.available_width();
    let inner_width = (width - 2.0 * KV_PAD_SIDE).max(0.0);
    // One column of labels for the whole box, so the values of every pair start
    // at the same left edge and a long value reads as one block.
    let column_left = label_column(ui, rows, inner_width);
    let pairs: Vec<Pair> = rows.iter().map(|row| pair(ui, row, inner_width, column_left)).collect();
    let height: f32 = pairs.iter().map(|pair| pair.height).sum();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, "Details"));

    let radius = CornerRadius::same(KV_RADIUS);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, theme::BG_ELEVATED());
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );

    let mut top = rect.min.y;
    for (row_index, (row, pair)) in rows.iter().zip(&pairs).enumerate() {
        let inner = Rect::from_min_max(
            Pos2::new(rect.min.x + KV_PAD_SIDE, top),
            Pos2::new(rect.max.x - KV_PAD_SIDE, top + pair.height),
        );
        let text_top = inner.min.y + KV_ROW_PAD_Y;
        let label_rect = Rect::from_min_size(Pos2::new(inner.min.x, text_top), pair.label.size());
        let value_rect = if one_line(&pair.value) {
            // A value of one line keeps the right edge of its row.
            Rect::from_min_size(
                Pos2::new(inner.max.x - pair.value.size().x, text_top),
                pair.value.size(),
            )
        } else {
            Rect::from_min_size(Pos2::new(inner.min.x + column_left, text_top), pair.value.size())
        };
        painter.galley(label_rect.min, pair.label.clone(), theme::FG_SOFT());
        painter.galley(value_rect.min, pair.value.clone(), theme::FG());

        text_node(ui, index, row_index, "label", label_rect, &row.label);
        text_node(ui, index, row_index, "value", value_rect, &row.value);

        if row_index + 1 < rows.len() {
            painter.hline(
                inner.x_range(),
                top + pair.height,
                Stroke::new(KV_SEPARATOR, theme::alpha(theme::BORDER_SUBTLE(), KV_SEPARATOR_ALPHA)),
            );
        }
        top += pair.height;
    }
}

/// The width of the column of labels, and the least gap that follows it: the
/// widest label of the box, inside the share of the row that a label may take.
fn label_column(ui: &Ui, rows: &[KvRow], inner_width: f32) -> f32 {
    let font = FontId::proportional(KV_LABEL_FONT);
    let widest = rows
        .iter()
        .map(|row| {
            text::line(ui.painter(), &row.label, &font, theme::FG_SOFT(), f32::INFINITY)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max);
    widest.min(inner_width * LABEL_SHARE) + KV_COLUMN_GAP
}

/// The label and the value of `row`, laid out inside a row `width` wide whose
/// values start at `column_left`.
fn pair(ui: &Ui, row: &KvRow, width: f32, column_left: f32) -> Pair {
    let painter = ui.painter();
    let label = text::wrapped(
        painter,
        &row.label,
        &FontId::proportional(KV_LABEL_FONT),
        theme::FG_SOFT(),
        (column_left - KV_COLUMN_GAP).max(0.0),
        None,
    );
    let value = text::wrapped(
        painter,
        &row.value,
        &value_font(row),
        theme::FG(),
        (width - column_left).max(0.0),
        None,
    );
    let text = label.size().y.max(value.size().y);
    Pair {
        label,
        value,
        height: (text + 2.0 * KV_ROW_PAD_Y).max(KV_ROW_HEIGHT),
    }
}

fn one_line(galley: &Galley) -> bool {
    galley.rows.len() <= 1
}

fn value_font(row: &KvRow) -> FontId {
    if row.mono {
        FontId::monospace(KV_MONO_FONT)
    } else {
        FontId::proportional(KV_VALUE_FONT)
    }
}

/// One painted text of a row as its own node, so a reader hears both halves
/// and a test can find them.
fn text_node(ui: &Ui, index: usize, row_index: usize, half: &'static str, rect: Rect, text_part: &str) {
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_kv", index, row_index, half)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text_part));
}
