//! Frame rendering: screencast JPEG → egui texture, letterboxed to the
//! panel body, plus placeholders for the non-ready states.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, StrokeKind, Ui, pos2, vec2};
use horizon_core::browser::{BrowserPanelState, BrowserStatus, PageScrollState};

use crate::browser_widget::BrowserUiState;

pub struct BodyOutput {
    pub image_rect: Option<Rect>,
    pub frame_size: Option<[f32; 2]>,
    pub viewport_size: Option<(u32, u32)>,
    /// This body's egui response owns the current pointer hit. Raw geometry
    /// alone is insufficient when browser panels overlap in different layers.
    pub pointer_target: bool,
    pub retry_clicked: bool,
    pub body_clicked: bool,
    /// Stable egui focus owner for page keyboard input.
    pub keyboard_focus_id: Option<egui::Id>,
}

/// Draw the body and return its separate layout and image geometry. The full
/// layout size drives Chrome's responsive viewport; the letterboxed image
/// rect is only for painting and pointer-coordinate mapping.
pub fn show_body(
    ui: &mut Ui,
    panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    interactive: bool,
) -> BodyOutput {
    let available = ui.available_rect_before_wrap();
    if available.size().x.min(available.size().y) < 24.0 {
        return BodyOutput {
            image_rect: None,
            frame_size: None,
            viewport_size: None,
            pointer_target: false,
            retry_clicked: false,
            body_clicked: false,
            keyboard_focus_id: None,
        };
    }
    // egui layout sizes are finite and non-negative.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let viewport_size = (available.width().round() as u32, available.height().round() as u32);
    let Some(data) = browser.frame_slot.latest() else {
        // No frame: the placeholder owns the body. It must be drawn before
        // allocating the body rect — allocating first would push the
        // placeholder's widgets past the clip rect (painter-drawn frames
        // are unaffected, which is why this only bites without frames).
        let retry = placeholder(ui, panel_id, browser, available, interactive);
        return BodyOutput {
            image_rect: None,
            frame_size: None,
            viewport_size: Some(viewport_size),
            pointer_target: false,
            retry_clicked: retry,
            body_clicked: false,
            keyboard_focus_id: None,
        };
    };
    let (body_rect, _) = ui.allocate_exact_size(available.size(), Sense::hover());
    let body_response = ui.interact(
        body_rect,
        ui.make_persistent_id(("browser_body", panel_id.0)),
        if interactive {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        },
    );
    let body_clicked = body_response.clicked();
    if body_clicked {
        body_response.request_focus();
    }
    let keyboard_focus_id = Some(body_response.id);
    let pointer_target = body_response.contains_pointer()
        || body_response.is_pointer_button_down_on()
        || body_response.drag_started()
        || body_response.dragged()
        // A press-drag-release can land fully inside one rendered frame with
        // the final pointer outside the body; the stopped-drag flag keeps
        // ownership for the release frame so the in-rect press is replayed.
        || body_response.drag_stopped();
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
    let Some(texture) = &state.texture else {
        return BodyOutput {
            image_rect: None,
            frame_size: Some(frame_size),
            viewport_size: Some(viewport_size),
            pointer_target,
            retry_clicked: false,
            body_clicked,
            keyboard_focus_id,
        };
    };

    // Letterbox (upscale allowed; linear filtering smooths it).
    let scale = (body_rect.width() / frame_size[0]).min(body_rect.height() / frame_size[1]);
    let rect = Rect::from_center_size(body_rect.center(), vec2(frame_size[0] * scale, frame_size[1] * scale));
    paint_browser_frame(ui, rect, texture);
    paint_webdriver_scrollbar(ui, rect, browser.frame_slot.page_scroll_state());

    BodyOutput {
        image_rect: Some(rect),
        frame_size: Some(frame_size),
        viewport_size: Some(viewport_size),
        pointer_target,
        retry_clicked: false,
        body_clicked,
        keyboard_focus_id,
    }
}

fn paint_browser_frame(ui: &Ui, rect: Rect, texture: &egui::TextureHandle) {
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
}

fn paint_webdriver_scrollbar(ui: &Ui, image_rect: Rect, state: Option<PageScrollState>) {
    let Some((track, thumb)) = state.and_then(|state| vertical_scrollbar_geometry(image_rect, state)) else {
        return;
    };
    ui.painter().rect_filled(
        track,
        CornerRadius::ZERO,
        crate::theme::alpha(crate::theme::PANEL_BG_ALT(), 220),
    );
    ui.painter()
        .rect_filled(thumb.shrink(2.0), CornerRadius::same(4), crate::theme::ACCENT());
}

fn vertical_scrollbar_geometry(image_rect: Rect, state: PageScrollState) -> Option<(Rect, Rect)> {
    if !state.is_valid() || state.content_height <= state.client_height + f32::EPSILON {
        return None;
    }
    let scale = image_rect.width() / state.viewport_width;
    let native_gutter = (state.viewport_width - state.client_width).max(0.0) * scale;
    let track_width = native_gutter.clamp(8.0, 14.0).min(image_rect.width());
    let track = Rect::from_min_max(
        pos2(image_rect.right() - track_width, image_rect.top()),
        image_rect.right_bottom(),
    );
    let visible_fraction = (state.client_height / state.content_height).clamp(0.0, 1.0);
    let natural_thumb_height = track.height() * visible_fraction;
    let thumb_height = if track.height() >= 24.0 {
        natural_thumb_height.clamp(24.0, track.height())
    } else {
        track.height()
    };
    let max_scroll = state.content_height - state.client_height;
    let progress = (state.scroll_y / max_scroll).clamp(0.0, 1.0);
    let thumb_top = track.top() + ((track.height() - thumb_height) * progress);
    let thumb = Rect::from_min_max(
        pos2(track.left(), thumb_top),
        pos2(track.right(), thumb_top + thumb_height),
    );
    Some((track, thumb))
}

fn placeholder(
    ui: &mut Ui,
    _panel_id: horizon_core::PanelId,
    browser: &mut BrowserPanelState,
    available: Rect,
    interactive: bool,
) -> bool {
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
            let retry_ready = browser.retry_ready();
            let response = ui
                .add_enabled(interactive && retry_ready, egui::Button::new("Retry"))
                .on_hover_text(if !interactive {
                    "Browser controls are unavailable in this view"
                } else if retry_ready {
                    "Restart the browser"
                } else {
                    "Waiting for the previous browser to finish shutting down"
                });
            if response.clicked() {
                retry_clicked = true;
            }
        }
    });
    retry_clicked
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};
    use horizon_core::browser::PageScrollState;

    use super::vertical_scrollbar_geometry;

    fn scroll_state(scroll_y: f32) -> PageScrollState {
        PageScrollState {
            scroll_x: 0.0,
            scroll_y,
            viewport_width: 1164.0,
            viewport_height: 608.0,
            client_width: 1152.0,
            client_height: 608.0,
            content_width: 1152.0,
            content_height: 3000.0,
        }
    }

    #[test]
    fn webdriver_scrollbar_overlay_tracks_native_gutter_and_scroll_position() {
        let image = Rect::from_min_max(pos2(0.0, 0.0), pos2(1164.0, 608.0));
        let Some((track, top_thumb)) = vertical_scrollbar_geometry(image, scroll_state(0.0)) else {
            panic!("scrollable page should have overlay geometry");
        };
        let Some((_, middle_thumb)) = vertical_scrollbar_geometry(image, scroll_state(1_196.0)) else {
            panic!("scrolled page should have overlay geometry");
        };

        assert!((track.width() - 12.0).abs() < f32::EPSILON);
        assert!((top_thumb.top() - image.top()).abs() < f32::EPSILON);
        assert!(middle_thumb.top() > top_thumb.top());
        assert!((top_thumb.height() - 123.2).abs() < 0.1);
    }
}
