//! A caption above a group of rows or a choice: upper case, small and dim.
//!
//! A caption names the group under it, so it takes no press and no focus. The
//! words are the agent's; the upper case is Horizon's drawing of them, so a
//! reader hears the words as the agent wrote them.

use egui::{FontId, Role, Sense, Ui, Vec2, WidgetInfo};

use crate::theme;

use super::metrics::{CAPTION_FONT, CAPTION_TRACKING};
use super::text;

/// Draw one `caption` component.
pub(super) fn show(ui: &mut Ui, _index: usize, text: &str) {
    let galley = text::spaced_line(
        ui.painter(),
        text,
        &FontId::proportional(CAPTION_FONT),
        theme::FG_DIM(),
        ui.available_width(),
        CAPTION_TRACKING,
    );
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), galley.size().y), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, text));
    ui.painter().galley(rect.min, galley, theme::FG_DIM());
}
