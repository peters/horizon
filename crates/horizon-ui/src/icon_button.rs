//! Square toolbar buttons with a painted mark.
//!
//! Each mark is drawn in a 16-point box inside the 22-point button.
//! Navigation glyphs on the same row stay 13 points. The button label is
//! the accessibility name. The caller supplies the hover text.

use egui::{
    Color32, CornerRadius, Painter, Pos2, Rect, Response, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType,
};

use crate::theme;

const BUTTON_SIZE: f32 = 22.0;
const ICON_SIZE: f32 = 16.0;
const MARK_STROKE: f32 = 1.35;

pub(crate) fn icon_button(
    ui: &mut Ui,
    enabled: bool,
    label: &str,
    draw: impl FnOnce(&Painter, Rect, Color32),
) -> Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .min_size(Vec2::splat(BUTTON_SIZE))
            .fill(theme::PANEL_BG_ALT())
            .corner_radius(6)
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE())),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, response.enabled(), label));
    let color = if response.enabled() {
        theme::FG()
    } else {
        theme::FG_DIM()
    };
    let icon = Rect::from_center_size(response.rect.center(), Vec2::splat(ICON_SIZE));
    draw(ui.painter(), icon, color);
    response
}

fn mark(color: Color32) -> Stroke {
    Stroke::new(MARK_STROKE, color)
}

fn at(rect: Rect, x: f32, y: f32) -> Pos2 {
    Pos2::new(rect.left() + x, rect.top() + y)
}

/// Filled camera. The lens is the button color, so this mark belongs on
/// [`icon_button`]. **Copy screenshot** puts the panel image on the clipboard.
pub(crate) fn paint_screenshot(painter: &Painter, rect: Rect, color: Color32) {
    let body = Rect::from_min_max(at(rect, 0.35, 4.2), at(rect, 15.65, 15.55));
    painter.rect_filled(body, CornerRadius::same(2), color);
    let housing = Rect::from_min_max(at(rect, 1.7, 0.85), at(rect, 6.45, 5.05));
    painter.rect_filled(housing, CornerRadius::same(1), color);
    let lens = Pos2::new(rect.left() + 9.7, rect.top() + 9.85);
    painter.circle_filled(lens, 3.15, theme::PANEL_BG_ALT());
    painter.circle_filled(Pos2::new(lens.x - 1.15, lens.y - 1.2), 0.75, color);
}

/// Rose disc while recording can start. Dim disc while the control is off.
pub(crate) fn paint_record(painter: &Painter, rect: Rect, enabled: bool) {
    let color = if enabled { theme::PALETTE_RED() } else { theme::FG_DIM() };
    painter.circle_filled(rect.center(), 4.05, color);
}

/// Filled square used for **Stop recording**.
pub(crate) fn paint_stop(painter: &Painter, rect: Rect, color: Color32) {
    let square = Rect::from_center_size(rect.center(), Vec2::splat(7.4));
    painter.rect_filled(square, CornerRadius::same(2), color);
}

/// Clipboard, for copying the private recording path.
pub(crate) fn paint_clipboard(painter: &Painter, rect: Rect, color: Color32) {
    let body = Rect::from_min_max(at(rect, 2.05, 3.7), at(rect, 13.95, 15.15));
    painter.rect_stroke(body, CornerRadius::same(2), mark(color), StrokeKind::Inside);
    let clip = Rect::from_center_size(at(rect, 8.0, 3.85), Vec2::new(5.4, 3.4));
    painter.rect_filled(clip, CornerRadius::same(1), color);
    let line = Stroke::new(1.2, color);
    painter.line_segment([at(rect, 4.55, 8.35), at(rect, 11.45, 8.35)], line);
    painter.line_segment([at(rect, 4.55, 11.05), at(rect, 9.25, 11.05)], line);
}
