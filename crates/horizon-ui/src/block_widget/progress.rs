//! The progress bar: a label row above a thin bar, and the detail under it in
//! the wide form.

use egui::{CornerRadius, FontId, Pos2, Rect, Role, Sense, Ui, Vec2, WidgetInfo};
use horizon_browser_control::block::BlockDocument;

use crate::theme;

use super::metrics::{
    LINE_FACTOR, PROGRESS_BAR_HEIGHT, PROGRESS_DETAIL_GAP, PROGRESS_LABEL_FONT, PROGRESS_LABEL_GAP, RADIUS_BAR,
};
use super::text;

/// The largest share of the label row the right-hand text may take.
const DETAIL_SHARE: f32 = 0.5;

/// Where the detail of a bar goes. The narrow card keeps it on the label row, and
/// the wide card gives it the line under the bar, where its words are not cut by
/// the label beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    /// The label at the left and the detail at the right of one line above the
    /// bar.
    Inline,
    /// The label above the bar and the detail on its own line under it.
    Stacked,
}

impl Layout {
    /// The height of one bar of this layout with its texts.
    const fn height(self) -> f32 {
        let label = PROGRESS_LABEL_FONT * LINE_FACTOR;
        let bar = label + PROGRESS_LABEL_GAP + PROGRESS_BAR_HEIGHT;
        match self {
            Self::Inline => bar,
            Self::Stacked => bar + PROGRESS_DETAIL_GAP + label,
        }
    }
}

/// Draw a bar of `percent` with the label row of the narrow card.
pub(super) fn show(ui: &mut Ui, index: usize, label: &str, percent: u8, detail: Option<&str>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, Layout::Inline.height()), Sense::hover());
    paint(ui, index, rect, label, percent, detail, Layout::Inline);
}

/// The layout of the bar beside the heading of a wide card: the detail takes its
/// own line when the bar has one, and a bar without a detail keeps the height of
/// the label row alone.
pub(super) const fn wide_layout(detail: Option<&str>) -> Layout {
    if detail.is_some() {
        Layout::Stacked
    } else {
        Layout::Inline
    }
}

/// The height of one bar with its texts, for the layout of the card form.
pub(super) fn height(layout: Layout) -> f32 {
    layout.height()
}

/// Paint a bar with its texts into `rect`, which the caller made
/// [`height`] tall: a wide card puts it beside the heading of its own row.
pub(super) fn paint(ui: &Ui, index: usize, rect: Rect, label: &str, percent: u8, detail: Option<&str>, layout: Layout) {
    let width = rect.width();
    let font = FontId::proportional(PROGRESS_LABEL_FONT);
    let response = ui.interact(rect, ui.make_persistent_id(("block_progress", index)), Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, label));

    let (bar_top, detail_top) = match layout {
        Layout::Inline => {
            paint_inline(ui, index, rect, label, detail, &font);
            let top = rect.min.y + progress_text_height();
            (top, top)
        }
        Layout::Stacked => {
            let painter = ui.painter();
            let label_galley = text::line(painter, label, &font, theme::FG_SOFT(), width);
            painter.galley(rect.min, label_galley, theme::FG_SOFT());
            let top = rect.min.y + progress_text_height();
            (top, top + PROGRESS_BAR_HEIGHT + PROGRESS_DETAIL_GAP)
        }
    };
    if layout == Layout::Stacked
        && let Some(detail) = detail
    {
        paint_detail(ui, index, rect, detail, &font, detail_top);
    }

    let painter = ui.painter();
    let track = Rect::from_min_size(
        Pos2::new(rect.min.x, bar_top + PROGRESS_LABEL_GAP),
        Vec2::new(width, PROGRESS_BAR_HEIGHT),
    );
    let radius = CornerRadius::same(RADIUS_BAR);
    painter.rect_filled(track, radius, theme::PANEL_BG_ALT());
    let filled = f32::from(percent.min(BlockDocument::MAX_PERCENT)) / f32::from(BlockDocument::MAX_PERCENT);
    if filled > 0.0 {
        let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * filled, PROGRESS_BAR_HEIGHT));
        painter.rect_filled(fill, radius, theme::ACCENT());
    }
}

/// The height of one line of the label font.
fn progress_text_height() -> f32 {
    PROGRESS_LABEL_FONT * LINE_FACTOR
}

/// Paints the label at the left and the detail at the right of one line above the
/// bar, which is the narrow form of the bar.
fn paint_inline(ui: &Ui, index: usize, rect: Rect, label: &str, detail: Option<&str>, font: &FontId) {
    let width = rect.width();
    let detail_galley = detail.map(|detail| {
        text::line(
            ui.painter(),
            detail,
            font,
            theme::FG_SOFT(),
            (width * DETAIL_SHARE).max(0.0),
        )
    });
    let detail_width = detail_galley.as_ref().map_or(0.0, |galley| galley.size().x);
    let label_galley = text::line(
        ui.painter(),
        label,
        font,
        theme::FG_SOFT(),
        (width - detail_width - PROGRESS_LABEL_GAP).max(0.0),
    );
    let label_height = label_galley
        .size()
        .y
        .max(detail_galley.as_ref().map_or(0.0, |galley| galley.size().y));

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
}

/// Paints the detail on its own line under the bar, at the right edge, as one
/// line that ends with an ellipsis, with its whole text while the pointer rests
/// on it.
fn paint_detail(ui: &Ui, index: usize, rect: Rect, detail: &str, font: &FontId, top: f32) {
    let galley = text::line(ui.painter(), detail, font, theme::FG_SOFT(), rect.width());
    let width = galley.size().x;
    ui.painter()
        .galley(Pos2::new(rect.max.x - width, top), galley, theme::FG_SOFT());
    let line = Rect::from_min_size(
        Pos2::new(rect.min.x, top),
        Vec2::new(rect.width(), progress_text_height()),
    );
    let response = ui.interact(
        line,
        ui.make_persistent_id(("block_progress_detail", index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, detail));
    response.on_hover_text(detail);
}

/// Registers the right-hand text of the narrow form as its own node.
fn label_node(ui: &Ui, index: usize, row: Rect, top: f32, detail: Option<&str>) {
    let Some(detail) = detail else { return };
    let rect = Rect::from_min_max(Pos2::new(row.min.x, row.min.y), Pos2::new(row.max.x, row.min.y + top));
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("block_progress_detail", index)),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, detail));
    response.on_hover_text(detail);
}
