//! Frame rendering: screencast JPEG → egui texture, letterboxed to the
//! panel body, plus placeholders for the non-ready states.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, StrokeKind, Ui, pos2, vec2};
use horizon_core::browser::{BrowserPanelState, BrowserStatus, NestedScrollbar, PageScrollState};

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
    let page_scroll = browser.frame_slot.page_scroll_state();
    paint_page_scrollbar(ui, rect, page_scroll);
    paint_nested_scrollbars(ui, rect, page_scroll, &browser.frame_slot.nested_scrollbars());
    let popup = browser.frame_slot.native_select_popup();
    if popup.is_none() {
        state.select_popup_dismissed = false;
    }
    super::select_popup::sync_ui_state(&mut state.select_popup, popup.as_deref());
    if let (Some(popup), Some(open)) = (popup.as_deref(), state.select_popup.as_mut()) {
        let _ = super::select_popup::show(ui, browser, rect, frame_size, popup, open);
    }

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

fn paint_page_scrollbar(ui: &Ui, image_rect: Rect, state: Option<PageScrollState>) {
    let Some((track, thumb)) = state.and_then(|state| vertical_scrollbar_geometry(image_rect, state)) else {
        return;
    };
    paint_scrollbar(&ui.painter_at(image_rect), track, thumb);
}

fn paint_nested_scrollbars(ui: &Ui, image_rect: Rect, page: Option<PageScrollState>, bars: &[NestedScrollbar]) {
    let Some(page) = page.filter(|page| !bars.is_empty() && page.is_valid()) else {
        return;
    };
    let painter = ui.painter_at(image_rect);
    for bar in bars {
        if let Some((track, thumb, visible)) = nested_scrollbar_geometry(image_rect, page, *bar) {
            paint_scrollbar(&painter.with_clip_rect(visible), track, thumb);
        }
    }
}

fn paint_scrollbar(painter: &egui::Painter, track: Rect, thumb: Rect) {
    painter.rect_filled(
        track,
        CornerRadius::ZERO,
        crate::theme::alpha(crate::theme::PANEL_BG_ALT(), 230),
    );
    painter.rect_filled(thumb.shrink(2.0), CornerRadius::same(4), crate::theme::ACCENT());
}

/// Map a nested container's CSS-pixel track, thumb and visible span onto the
/// letterboxed frame.
fn nested_scrollbar_geometry(
    image_rect: Rect,
    page: PageScrollState,
    bar: NestedScrollbar,
) -> Option<(Rect, Rect, Rect)> {
    let (thumb_y, thumb_height) = bar.thumb()?;
    let scale = vec2(
        image_rect.width() / page.viewport_width,
        image_rect.height() / page.viewport_height,
    );
    let track = Rect::from_min_size(
        image_rect.min + vec2(bar.track_x * scale.x, bar.track_y * scale.y),
        vec2(bar.track_width * scale.x, bar.track_height * scale.y),
    );
    let thumb = Rect::from_min_size(
        pos2(track.left(), track.top() + thumb_y * scale.y),
        vec2(track.width(), thumb_height * scale.y),
    );
    let visible = Rect::from_x_y_ranges(
        track.x_range(),
        image_rect.top() + bar.visible_top * scale.y..=image_rect.top() + bar.visible_bottom * scale.y,
    );
    Some((track, thumb, visible))
}

fn vertical_scrollbar_geometry(image_rect: Rect, state: PageScrollState) -> Option<(Rect, Rect)> {
    let overlay = state.vertical_overlay()?;
    if state.viewport_width <= f32::EPSILON || state.viewport_height <= f32::EPSILON {
        return None;
    }
    let scale_x = image_rect.width() / state.viewport_width;
    let scale_y = image_rect.height() / state.viewport_height;
    let track_width = (overlay.track_width * scale_x).min(image_rect.width());
    let track = Rect::from_min_max(
        pos2(image_rect.right() - track_width, image_rect.top()),
        image_rect.right_bottom(),
    );
    let thumb_height = (overlay.thumb_height * scale_y).clamp(0.0, track.height());
    let thumb_top = track.top() + (overlay.thumb_y * scale_y);
    let thumb = Rect::from_min_max(
        pos2(track.left(), thumb_top),
        pos2(track.right(), (thumb_top + thumb_height).min(track.bottom())),
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
        if let Some(note) = browser.remote_status.as_deref() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(note).color(crate::theme::FG_SOFT()).size(12.0));
        }
        if !browser.status.is_alive() {
            ui.add_space(12.0);
            let can_retry = browser.can_retry();
            let retry_ready = can_retry && browser.retry_ready();
            let response = ui
                .add_enabled(interactive && retry_ready, egui::Button::new("Retry"))
                .on_hover_text(if !interactive {
                    "Browser controls are unavailable in this view"
                } else if !can_retry {
                    "This remote session ended with a previous run; create the panel again"
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
    use horizon_core::browser::{NestedScrollbar, PageScrollState};

    use super::{nested_scrollbar_geometry, vertical_scrollbar_geometry};

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
    fn page_scrollbar_overlay_tracks_native_gutter_and_scroll_position() {
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

    #[test]
    fn nested_scrollbar_maps_its_css_gutter_onto_a_scaled_frame() {
        let image = Rect::from_min_max(pos2(100.0, 50.0), pos2(100.0 + 582.0, 50.0 + 304.0));
        let bar = NestedScrollbar {
            track_x: 1_152.0,
            track_y: 56.0,
            track_width: 12.0,
            track_height: 552.0,
            visible_top: 100.0,
            visible_bottom: 608.0,
            scroll_top: 0.0,
            scroll_height: 5_520.0,
        };
        let Some((track, thumb, visible)) = nested_scrollbar_geometry(image, scroll_state(0.0), bar) else {
            panic!("scrollable container should have overlay geometry");
        };
        assert!((track.left() - 676.0).abs() < 0.01);
        assert!((track.top() - 78.0).abs() < 0.01);
        assert!((track.width() - 6.0).abs() < 0.01);
        assert!((track.height() - 276.0).abs() < 0.01);
        assert!((thumb.top() - track.top()).abs() < 0.01);
        assert!((thumb.height() - 27.6).abs() < 0.01);
        assert!((visible.top() - 100.0).abs() < 0.01);
        assert!((visible.bottom() - track.bottom()).abs() < 0.01);
        assert!((visible.left() - track.left()).abs() < 0.01);

        let Some((_, bottom, _)) = nested_scrollbar_geometry(
            image,
            scroll_state(0.0),
            NestedScrollbar {
                scroll_top: 4_968.0,
                ..bar
            },
        ) else {
            panic!("scrolled container should keep overlay geometry");
        };
        assert!((bottom.bottom() - track.bottom()).abs() < 0.01);
    }
}
