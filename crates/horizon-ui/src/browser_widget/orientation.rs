//! Remote device rotation controls with measured, rather than requested, selection.
use crate::theme;
use egui::{Button, RichText, Ui};
use horizon_core::browser::{
    BrowserPanelState, BrowserStatus,
    remote::{OrientationSupport, RemoteOrientation},
};

pub(super) fn show(ui: &mut Ui, browser: &mut BrowserPanelState, interactive: bool) -> bool {
    if !browser.is_remote() {
        return false;
    }
    let enabled = interactive
        && matches!(browser.status, BrowserStatus::Ready)
        && browser.teach().is_none()
        && browser.orientation.pending.is_none()
        && browser.orientation.state.support != OrientationSupport::Unsupported;
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        for (orientation, label) in [
            (RemoteOrientation::Portrait, "Portrait"),
            (RemoteOrientation::Landscape, "Landscape"),
        ] {
            let response = ui.add_enabled(
                enabled,
                Button::new(label).selected(browser.orientation.state.applied == Some(orientation)),
            );
            if response
                .on_hover_text("Rotate the remote device; selection updates after device and page geometry agree")
                .clicked()
            {
                clicked = true;
                browser.request_orientation(orientation);
            }
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
    });
    if let Some(error) = &browser.orientation.error {
        ui.label(RichText::new(error).size(10.5).color(theme::PALETTE_RED()));
    }
    clicked
}
