//! The progress bar: a label row above a thin bar.

use egui::{CornerRadius, FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::BlockDocument;

use crate::theme;

use super::metrics::{PROGRESS_BAR_HEIGHT, PROGRESS_LABEL_FONT, PROGRESS_LABEL_GAP, PROGRESS_RADIUS};
use super::text;

/// The largest share of the label row the right-hand text may take.
const DETAIL_SHARE: f32 = 0.5;

/// Draw a bar of `percent` under a label row that holds `label` on the left and
/// `detail` on the right.
pub(super) fn show(ui: &mut Ui, index: usize, label: &str, percent: u8, detail: Option<&str>) {
    let width = ui.available_width();
    let font = FontId::proportional(PROGRESS_LABEL_FONT);
    let detail_galley = detail.map(|detail| {
        text::line(
            ui.painter(),
            detail,
            &font,
            theme::FG_SOFT(),
            (width * DETAIL_SHARE).max(0.0),
        )
    });
    let detail_width = detail_galley.as_ref().map_or(0.0, |galley| galley.size().x);
    let label_galley = text::line(
        ui.painter(),
        label,
        &font,
        theme::FG_SOFT(),
        (width - detail_width - PROGRESS_LABEL_GAP).max(0.0),
    );
    let label_height = label_galley
        .size()
        .y
        .max(detail_galley.as_ref().map_or(0.0, |galley| galley.size().y));
    let height = label_height + PROGRESS_LABEL_GAP + PROGRESS_BAR_HEIGHT;

    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, label));

    let painter = ui.painter();
    painter.galley(rect.min, label_galley, theme::FG_SOFT());
    if let Some(galley) = detail_galley {
        let detail_height = galley.size().y;
        let detail_width = galley.size().x;
        painter.galley(
            Pos2::new(rect.max.x - detail_width, rect.min.y + label_height - detail_height),
            galley,
            theme::FG_SOFT(),
        );
        label_node(ui, index, rect, label_height, detail);
    }

    let track = Rect::from_min_size(
        Pos2::new(rect.min.x, rect.min.y + label_height + PROGRESS_LABEL_GAP),
        Vec2::new(width, PROGRESS_BAR_HEIGHT),
    );
    let radius = CornerRadius::same(PROGRESS_RADIUS);
    painter.rect_filled(track, radius, theme::PANEL_BG_ALT());
    let filled = f32::from(percent.min(BlockDocument::MAX_PERCENT)) / f32::from(BlockDocument::MAX_PERCENT);
    if filled > 0.0 {
        let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * filled, PROGRESS_BAR_HEIGHT));
        painter.rect_filled(fill, radius, theme::ACCENT());
    }
}

/// Registers the right-hand text as its own node.
fn label_node(ui: &Ui, index: usize, row: Rect, top: f32, detail: Option<&str>) {
    let Some(detail) = detail else { return };
    let rect = Rect::from_min_max(Pos2::new(row.min.x, row.min.y), Pos2::new(row.max.x, row.min.y + top));
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_progress_detail", index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, detail));
}
