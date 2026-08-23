//! Frame rendering: screencast JPEG → egui texture, letterboxed to the
//! panel body, plus placeholders for the non-ready states.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, StrokeKind, Ui, pos2, vec2};
use horizon_core::browser::{BrowserPanelState, BrowserStatus};

use crate::browser_widget::BrowserUiState;

/// Draw the body. Returns the drawn image rect (for input mapping), the
/// emulated viewport size in CSS pixels, whether the retry button was
/// clicked, and whether the body itself was clicked this frame (for
/// panel-focus requests).
pub fn show_body(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
) -> (Option<Rect>, Option<[f32; 2]>, bool, bool) {
    let available = ui.available_rect_before_wrap();
    if available.size().x.min(available.size().y) < 24.0 {
        return (None, None, false, false);
    }
    let slot = browser.frame_slot.latest();
    let Some(data) = &slot.data else {
        drop(slot);
        // No frame: the placeholder owns the body. It must be drawn before
        // allocating the body rect — allocating first would push the
        // placeholder's widgets past the clip rect (painter-drawn frames
        // are unaffected, which is why this only bites without frames).
        let retry = placeholder(ui, panel_id, browser, available);
        return (None, None, retry, false);
    };
    let body_response = ui.allocate_rect(available, Sense::click());
    let body_clicked = body_response.clicked();
    let width = data.width as usize;
    let height = data.height as usize;
    let frame_size = [
        f32::from(u16::try_from(width).unwrap_or(0xFFFF)),
        f32::from(u16::try_from(height).unwrap_or(0xFFFF)),
    ];

    // Update the texture only when a new frame actually arrived —
    // screencasts are change-driven, so idle pages do no work here.
    if state.seq != data.seq {
        let image = egui::epaint::ColorImage::from_rgb([width, height], &data.rgb);
        let options = egui::TextureOptions::LINEAR;
        let resize_needed = state.texture.as_ref().is_some_and(|t| t.size() != [width, height]);
        if state.texture.is_none() || resize_needed {
            let name = format!("browser-{}-{}", browser.panel_local_id, panel_id.0);
            state.texture = Some(ui.ctx().load_texture(name, image, options));
        } else if let Some(handle) = state.texture.as_mut() {
            handle.set(image, options);
        }
        state.seq = data.seq;
    }
    drop(slot);

    let Some(texture) = &state.texture else {
        return (None, Some(frame_size), false, body_clicked);
    };

    // Letterbox (upscale allowed; linear filtering smooths it).
    let scale = (available.width() / frame_size[0]).min(available.height() / frame_size[1]);
    let rect = Rect::from_center_size(available.center(), vec2(frame_size[0] * scale, frame_size[1] * scale));
    ui.painter().add(egui::epaint::Shape::Rect(egui::epaint::RectShape {
        rect,
        corner_radius: CornerRadius::ZERO,
        fill: Color32::WHITE,
        stroke: egui::Stroke::default(),
        stroke_kind: StrokeKind::Inside,
        round_to_pixels: None,
        blur_width: 0.0,
        brush: Some(Arc::new(egui::epaint::Brush {
            fill_texture_id: texture.id(),
            uv: Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        })),
        angle: 0.0,
    }));

    (Some(rect), Some(frame_size), false, body_clicked)
}

fn placeholder(ui: &mut Ui, _panel_id: horizon_core::PanelId, browser: &BrowserPanelState, available: Rect) -> bool {
    let message = match &browser.status {
        BrowserStatus::Starting => "Starting browser…".to_string(),
        BrowserStatus::Ready => "Waiting for frames…".to_string(),
        BrowserStatus::Error { message } => format!("Browser error: {message}"),
        BrowserStatus::Stopped { code } => format!(
            "Browser stopped{}",
            code.map(|c| format!(" (exit {c})")).unwrap_or_default()
        ),
    };
    let mut retry_clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(((available.height() - 90.0).max(0.0)) / 2.0);
        ui.label(egui::RichText::new(message).color(crate::theme::FG_DIM()));
        if !browser.status.is_alive() {
            ui.add_space(12.0);
            let response = ui.add(egui::Button::new("Retry")).on_hover_text("Restart the browser");
            if response.clicked() {
                retry_clicked = true;
            }
        }
    });
    retry_clicked
}
