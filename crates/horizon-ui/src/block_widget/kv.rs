//! The label and value box: an inset surface with one pair per row.

use egui::{CornerRadius, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::KvRow;

use crate::theme;

use super::metrics::{
    KV_COLUMN_GAP, KV_LABEL_FONT, KV_MONO_FONT, KV_PAD_SIDE, KV_RADIUS, KV_ROW_HEIGHT, KV_SEPARATOR,
    KV_SEPARATOR_ALPHA, KV_VALUE_FONT, count,
};
use super::text;

/// The largest share of a row a value may take, so a long value never pushes
/// its label off the row.
const VALUE_SHARE: f32 = 0.65;

/// Draw `rows` as one inset box.
pub(super) fn show(ui: &mut Ui, index: usize, rows: &[KvRow]) {
    if rows.is_empty() {
        return;
    }
    let width = ui.available_width();
    let height = count(rows.len()) * KV_ROW_HEIGHT;
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

    for (row_index, row) in rows.iter().enumerate() {
        let top = rect.min.y + count(row_index) * KV_ROW_HEIGHT;
        let inner = Rect::from_min_max(
            Pos2::new(rect.min.x + KV_PAD_SIDE, top),
            Pos2::new(rect.max.x - KV_PAD_SIDE, top + KV_ROW_HEIGHT),
        );
        let value_font = value_font(row);
        let value = text::line(
            ui.painter(),
            &row.value,
            &value_font,
            theme::FG(),
            (inner.width() * VALUE_SHARE).max(0.0),
        );
        let value_width = value.size().x;
        let label = text::line(
            ui.painter(),
            &row.label,
            &FontId::proportional(KV_LABEL_FONT),
            theme::FG_SOFT(),
            (inner.width() - value_width - KV_COLUMN_GAP).max(0.0),
        );

        let value_height = value.size().y;
        painter.galley(
            Pos2::new(inner.max.x - value_width, inner.center().y - value_height * 0.5),
            value,
            theme::FG(),
        );
        let label_height = label.size().y;
        let label_x = inner.min.x;
        let label_y = inner.center().y - label_height * 0.5;
        painter.galley(Pos2::new(label_x, label_y), label, theme::FG_SOFT());

        label_node(ui, index, row_index, inner, "label", &row.label);
        label_node(ui, index, row_index, inner, "value", &row.value);

        if row_index + 1 < rows.len() {
            painter.hline(
                inner.x_range(),
                top + KV_ROW_HEIGHT,
                Stroke::new(KV_SEPARATOR, theme::alpha(theme::BORDER_SUBTLE(), KV_SEPARATOR_ALPHA)),
            );
        }
    }
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
fn label_node(ui: &Ui, index: usize, row_index: usize, row: Rect, half: &'static str, text_part: &str) {
    let rect = Rect::from_min_max(Pos2::new(row.min.x, row.min.y), Pos2::new(row.max.x, row.max.y));
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_kv", index, row_index, half)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text_part));
}
