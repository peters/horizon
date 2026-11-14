//! The status frame: what the host says about a card it owns.
//!
//! Two frames use it. A card whose agent left shows a warning frame that
//! explains why its buttons and rows are dead, and a card that refused a click
//! shows the same frame in the info tone. The text of both frames is Horizon's,
//! so an agent cannot dress up its own words as a host message.

use egui::{CornerRadius, FontId, Pos2, Rect, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::Tone;

use crate::theme;

use super::metrics::{
    RADIUS_CONTROL, STATUS_LINE_FONT, STATUS_LINE_GAP, STATUS_PAD_SIDE, STATUS_PAD_Y, STATUS_TITLE_FONT,
};
use super::text;
use super::tones;

/// Line height of the second line, as a multiple of its font size.
const LINE_HEIGHT: f32 = 1.45;

/// Draw one status frame and return the height it took.
pub(super) fn show(ui: &mut Ui, tag: &'static str, tone: Tone, title: &str, line: &str) -> f32 {
    let width = ui.available_width();
    let inner_width = (width - 2.0 * STATUS_PAD_SIDE).max(0.0);
    let colours = tones::status(tone);
    let title_galley = text::wrapped(
        ui.painter(),
        title,
        &FontId::proportional(STATUS_TITLE_FONT),
        colours.text,
        inner_width,
        None,
    );
    let line_galley = text::wrapped(
        ui.painter(),
        line,
        &FontId::proportional(STATUS_LINE_FONT),
        theme::FG_SOFT(),
        inner_width,
        Some(STATUS_LINE_FONT * LINE_HEIGHT),
    );
    let title_height = title_galley.size().y;
    let line_height = line_galley.size().y;
    let height = 2.0 * STATUS_PAD_Y + title_height + STATUS_LINE_GAP + line_height;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, title));

    let radius = CornerRadius::same(RADIUS_CONTROL);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, colours.fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, colours.outline), StrokeKind::Inside);
    let top = rect.min.y + STATUS_PAD_Y;
    painter.galley(Pos2::new(rect.min.x + STATUS_PAD_SIDE, top), title_galley, colours.text);
    let line_top = top + title_height + STATUS_LINE_GAP;
    painter.galley(
        Pos2::new(rect.min.x + STATUS_PAD_SIDE, line_top),
        line_galley,
        theme::FG_SOFT(),
    );
    let node = Rect::from_min_max(
        Pos2::new(rect.min.x + STATUS_PAD_SIDE, line_top),
        Pos2::new(rect.max.x - STATUS_PAD_SIDE, line_top + line_height),
    );
    let response = ui.interact(node, ui.make_persistent_id(("block_status_line", tag)), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, line));
    height
}
