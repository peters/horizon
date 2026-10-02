//! Remote device rotation controls with measured, rather than requested, selection.
use crate::theme;
use egui::{Button, Rect, RichText, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType, vec2};
use horizon_core::browser::{
    BrowserPanelState, BrowserStatus,
    remote::{OrientationSupport, RemoteOrientation},
};

pub(super) fn controls(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    if browser.remote_target().is_none() {
        return false;
    }
    let enabled = interactive
        && matches!(browser.status, BrowserStatus::Ready)
        && browser.teach().is_none()
        && browser.orientation.pending.is_none()
        && browser.orientation.state.support != OrientationSupport::Unsupported;
    ui.add_enabled_ui(enabled, |ui| {
        let mut clicked = false;
        for (orientation, label, tooltip) in [
            (
                RemoteOrientation::Portrait,
                "Portrait",
                "Portrait: rotate the remote device upright",
            ),
            (
                RemoteOrientation::Landscape,
                "Landscape",
                "Landscape: rotate the remote device sideways",
            ),
        ] {
            let selected = browser.orientation.state.applied == Some(orientation);
            let response = ui.add(
                Button::new("")
                    .min_size(vec2(26.0, 26.0))
                    .corner_radius(6)
                    .selected(selected),
            );
            response.widget_info(|| WidgetInfo::selected(WidgetType::Button, response.enabled(), selected, label));
            let icon_size = match orientation {
                RemoteOrientation::Portrait => vec2(10.0, 16.0),
                RemoteOrientation::Landscape => vec2(16.0, 10.0),
            };
            let icon = Rect::from_center_size(response.rect.center(), icon_size);
            let color = ui.style().interact_selectable(&response, selected).fg_stroke.color;
            let painter = ui.painter_at(response.rect);
            painter.rect_stroke(icon, 2, Stroke::new(1.5, color), StrokeKind::Inside);
            let center = icon.center();
            let indicator = match orientation {
                RemoteOrientation::Portrait => [
                    egui::pos2(center.x - 2.0, icon.bottom() - 3.0),
                    egui::pos2(center.x + 2.0, icon.bottom() - 3.0),
                ],
                RemoteOrientation::Landscape => [
                    egui::pos2(icon.right() - 3.0, center.y - 2.0),
                    egui::pos2(icon.right() - 3.0, center.y + 2.0),
                ],
            };
            painter.line_segment(indicator, Stroke::new(1.0, color));
            if response.on_hover_text(tooltip).clicked() {
                clicked = true;
                browser.request_orientation(orientation);
            }
        }
        clicked
    })
    .inner
}

pub(super) fn status(ui: &mut Ui, browser: &BrowserPanelState) {
    if browser.remote_target().is_none() {
        return;
    }
    let status = if browser.orientation.pending.is_some() {
        "Rotating…"
    } else {
        match browser.orientation.state.support {
            OrientationSupport::Unsupported => "Unsupported",
            OrientationSupport::Unverified => "Unverified",
            OrientationSupport::Supported if browser.orientation.state.applied.is_none() => "Unverified",
            OrientationSupport::Supported => "Verified",
        }
    };
    ui.label(RichText::new(status).size(10.5));
    if let Some(error) = &browser.orientation.error {
        ui.label(RichText::new(error).size(10.5).color(theme::PALETTE_RED()));
    }
}
