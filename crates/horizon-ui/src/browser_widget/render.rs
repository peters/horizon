//! Frame rendering: screencast JPEG → egui texture, letterboxed to the
//! panel body, plus placeholders for the non-ready states.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, StrokeKind, Ui, pos2, vec2};
use horizon_core::browser::{BrowserPanelState, BrowserStatus, PageScrollState};

use crate::browser_widget::BrowserUiState;
use crate::frame_budget::{FRAME_BUDGET_SIZE, MAX_FRAME_PIXELS};

/// Smallest viewport side `synchronize_viewport` accepts as a real layout.
const MIN_STABLE_VIEWPORT_SIDE: f32 = 33.0;

pub struct BodyOutput {
    pub image_rect: Option<Rect>,
    /// Exact menu bounds painted from this frame's popup snapshot.
    pub native_select_menu: Option<Rect>,
    pub frame_size: Option<[f32; 2]>,
    pub viewport_size: Option<(u32, u32)>,
    /// The body's own size, before panel zoom. A pinned viewport keeps its
    /// backend-owned frame size, so the geometry remembered for a later reset
    /// must be the panel's real size rather than a zoomed layout size.
    pub host_viewport_size: Option<(u32, u32)>,
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
            native_select_menu: None,
            frame_size: None,
            viewport_size: None,
            host_viewport_size: None,
            pointer_target: false,
            retry_clicked: false,
            body_clicked: false,
            keyboard_focus_id: None,
        };
    }
    let (viewport_size, host_viewport_size) = sync_viewport_sizes(ui, state, available.size());
    let Some(data) = browser.frame_slot.latest() else {
        // No frame: the placeholder owns the body. It must be drawn before
        // allocating the body rect — allocating first would push the
        // placeholder's widgets past the clip rect (painter-drawn frames
        // are unaffected, which is why this only bites without frames).
        let retry = placeholder(ui, panel_id, browser, available, interactive);
        return BodyOutput {
            image_rect: None,
            native_select_menu: None,
            frame_size: None,
            viewport_size: Some(viewport_size),
            host_viewport_size: Some(host_viewport_size),
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
            native_select_menu: None,
            frame_size: Some(frame_size),
            viewport_size: Some(viewport_size),
            host_viewport_size: Some(host_viewport_size),
            pointer_target,
            retry_clicked: false,
            body_clicked,
            keyboard_focus_id,
        };
    };

    // Letterbox (upscale allowed; linear filtering smooths it).
    let responsive = super::supports_panel_zoom(browser);
    let scale = frame_scale(body_rect.size(), vec2(frame_size[0], frame_size[1]), responsive);
    let rect = Rect::from_center_size(body_rect.center(), vec2(frame_size[0] * scale, frame_size[1] * scale));
    paint_browser_frame(ui, rect, texture);
    paint_page_scrollbar(ui, rect, browser.frame_slot.page_scroll_state());
    let native_select_menu = show_native_select(ui, browser, state, rect, frame_size);

    BodyOutput {
        image_rect: Some(rect),
        native_select_menu,
        frame_size: Some(frame_size),
        viewport_size: Some(viewport_size),
        host_viewport_size: Some(host_viewport_size),
        pointer_target,
        retry_clicked: false,
        body_clicked,
        keyboard_focus_id,
    }
}

fn show_native_select(
    ui: &mut Ui,
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    rect: Rect,
    frame_size: [f32; 2],
) -> Option<Rect> {
    let popup = browser.frame_slot.native_select_popup();
    if popup.is_none() {
        state.select_popup_dismissed = false;
    }
    super::select_popup::sync_ui_state(&mut state.select_popup, popup.as_deref());
    let (Some(popup), Some(open)) = (popup.as_deref(), state.select_popup.as_mut()) else {
        return None;
    };
    super::select_popup::show(ui, browser, rect, frame_size, popup, open).map(|layout| layout.menu)
}

pub(super) fn route_zoom(
    ui: &Ui,
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    body: &BodyOutput,
    fullscreen: bool,
) {
    if resolve_zoom_owner(ui, browser, body.native_select_menu, fullscreen) && body.native_select_menu.is_some() {
        browser.send(horizon_core::browser::BrowserCommand::NativeSelectDismiss);
        state.select_popup_dismissed = true;
    }
    // Chrome and placeholders use the same body ownership as the canvas.
    apply_zoom_gesture(ui, browser, state, crate::panel_zoom::owns_gesture(ui));
}

/// Page zoom lays the body out in fewer (or more) CSS pixels than it occupies
/// on screen; the frame that comes back is letterboxed to the body, so the
/// page reflows and scales exactly like browser zoom.
///
/// A zoom that would push an axis under `MIN_STABLE_VIEWPORT_SIDE` is capped:
/// `synchronize_viewport` treats such a viewport as a transient layout and
/// never sends it, which would leave the page at its previous scale with the
/// selected zoom doing nothing at all. A small panel zooms as far as it can
/// instead.
fn zoomed_viewport(available: egui::Vec2, zoom: f32, max_texture_side: usize) -> (u32, u32) {
    let zoom = effective_zoom(available, zoom, max_texture_side);
    // An extreme aspect ratio can want a scale that satisfies the texture
    // limit on one axis and breaks the minimum viewport on the other. One
    // scale cannot serve both, so each axis is clamped into the sendable
    // range and the frame is letterboxed into the body as usual — a slightly
    // different aspect beats a viewport the backend sync refuses to send.
    let side_limit = side_limit(max_texture_side);
    let width = (available.x / zoom).clamp(MIN_STABLE_VIEWPORT_SIDE, side_limit);
    let height = (available.y / zoom).clamp(MIN_STABLE_VIEWPORT_SIDE, side_limit);
    let (mut width, mut height) = rounded_viewport(vec2(width, height));
    // Independent axis rounding can exceed the float clamp's pixel budget.
    // Trimming the longer axis removes the fewest pixels at this boundary.
    if u64::from(width) * u64::from(height) > u64::from(MAX_FRAME_PIXELS) {
        if width >= height {
            width = MAX_FRAME_PIXELS / height;
        } else {
            height = MAX_FRAME_PIXELS / width;
        }
    }
    (width, height)
}

// egui layout sizes are finite and non-negative.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_viewport(size: egui::Vec2) -> (u32, u32) {
    (size.x.round() as u32, size.y.round() as u32)
}

#[allow(clippy::cast_precision_loss)]
fn side_limit(max_texture_side: usize) -> f32 {
    (max_texture_side.max(1) as f32).max(MIN_STABLE_VIEWPORT_SIDE)
}

/// Record the zoom this panel can really apply, and report the emulated
/// viewport for it alongside the panel's own unzoomed size.
fn sync_viewport_sizes(ui: &Ui, state: &mut BrowserUiState, available: egui::Vec2) -> ((u32, u32), (u32, u32)) {
    let max_texture_side = ui.ctx().input(|input| input.max_texture_side);
    let viewport = zoomed_viewport(available, state.zoom.factor(), max_texture_side);
    let frame_size = vec2(
        f32::from(u16::try_from(viewport.0).unwrap_or(u16::MAX)),
        f32::from(u16::try_from(viewport.1).unwrap_or(u16::MAX)),
    );
    let effective_zoom = crate::panel_zoom::PanelZoom::new(frame_scale(available, frame_size, true));
    if state.effective_zoom != effective_zoom {
        state.effective_zoom = effective_zoom;
        // The toolbar already painted the previous scale before laying out the body.
        ui.ctx().request_repaint();
    }
    (viewport, rounded_viewport(available))
}

/// An oversized body may need a smaller frame than even 400% zoom permits.
/// Keep the frame budget and letterbox the remaining space instead of silently
/// magnifying beyond the selector's maximum. Fixed viewports still fit normally.
fn frame_scale(available: egui::Vec2, frame_size: egui::Vec2, responsive: bool) -> f32 {
    let fitted = (available.x / frame_size.x).min(available.y / frame_size.y);
    if responsive {
        fitted.min(crate::panel_zoom::MAX_ZOOM)
    } else {
        fitted
    }
}

/// The zoom a panel can actually apply. Zooming in is capped where the
/// viewport would drop under the size `synchronize_viewport` accepts, and
/// zooming out where the frame would pass the renderer's texture side limit
/// or the pixel budget above; the panel reports this scale so the control
/// shows what is really on screen.
fn effective_zoom(available: egui::Vec2, zoom: f32, max_texture_side: usize) -> f32 {
    let side_limit = side_limit(max_texture_side);
    let pixel_budget = f32::from(FRAME_BUDGET_SIZE[0]) * f32::from(FRAME_BUDGET_SIZE[1]);
    let floor = (available.x / side_limit)
        .max(available.y / side_limit)
        .max((available.x * available.y / pixel_budget).sqrt())
        .max(crate::panel_zoom::MIN_ZOOM);
    let ceiling = (available.min_elem() / MIN_STABLE_VIEWPORT_SIDE).max(floor);
    zoom.clamp(floor, ceiling)
}

/// Finish routing with the menu actually painted, after variable-height chrome
/// and current frame fitting have established its geometry.
pub(super) fn resolve_zoom_owner(ui: &Ui, browser: &BrowserPanelState, menu: Option<Rect>, fullscreen: bool) -> bool {
    let contains_pointer =
        menu.is_some_and(|rect| crate::panel_zoom::local_pointer(ui).is_some_and(|point| rect.contains(point)));
    let canvas_fallback =
        !fullscreen && !super::supports_panel_zoom(browser) && crate::panel_zoom::is_native_pinch(ui.ctx());
    crate::panel_zoom::resolve_content_owner(ui, contains_pointer, canvas_fallback)
}

pub(super) fn apply_zoom_gesture(ui: &Ui, browser: &BrowserPanelState, state: &mut BrowserUiState, hovered: bool) {
    if !super::supports_panel_zoom(browser) {
        return;
    }
    let Some(delta) = crate::panel_zoom::gesture_delta(ui, hovered) else {
        return;
    };
    // Act on the scale actually on screen. A selection a resource limit had
    // to cap is kept only while the gesture pushes further past that cap, so
    // coming back toward the usable range responds immediately instead of
    // walking a hidden value.
    let requested = state.zoom.factor();
    let effective = state.effective_zoom.factor();
    let pushes_past_cap = (requested > effective && delta > 1.0) || (requested < effective && delta < 1.0);
    let base = if pushes_past_cap { requested } else { effective };
    let Some(zoomed) = crate::panel_zoom::gesture_target(base, delta) else {
        return;
    };
    if zoomed == state.zoom {
        return;
    }
    state.zoom = zoomed;
    // This frame already laid the body out at the previous scale, and a static
    // page produces no frames of its own: ask for the one that resends the
    // emulated viewport.
    ui.ctx().request_repaint();
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
    ui.painter().rect_filled(
        track,
        CornerRadius::ZERO,
        crate::theme::alpha(crate::theme::PANEL_BG_ALT(), 230),
    );
    ui.painter()
        .rect_filled(thumb.shrink(2.0), CornerRadius::same(4), crate::theme::ACCENT());
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
mod tests;
