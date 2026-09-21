//! Frame rendering: screencast JPEG → egui texture, letterboxed to the
//! panel body, plus placeholders for the non-ready states.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, Sense, StrokeKind, Ui, pos2, vec2};
use horizon_core::browser::{BrowserPanelState, BrowserStatus, PageScrollState};

use crate::browser_widget::BrowserUiState;

/// Smallest viewport side `synchronize_viewport` accepts as a real layout.
const MIN_STABLE_VIEWPORT_SIDE: f32 = 33.0;
/// Frames arrive at the emulated viewport's size, are decoded to RGB and
/// uploaded as one texture, so zooming out is bounded by a 4K-class pixel
/// budget as well as the renderer's own side limit.
const FRAME_BUDGET_SIZE: [u16; 2] = [3840, 2160];
const MAX_FRAME_PIXELS: u32 = FRAME_BUDGET_SIZE[0] as u32 * FRAME_BUDGET_SIZE[1] as u32;

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
mod tests {
    use egui::{Rect, pos2};
    use horizon_core::browser::PageScrollState;

    use super::{BrowserUiState, apply_zoom_gesture, vertical_scrollbar_geometry};

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

    fn zoom_menu() -> horizon_core::browser::NativeSelectPopup {
        use horizon_core::browser::{BrowserBounds, NativeSelectOption, NativeSelectPopup};
        NativeSelectPopup {
            css_path: "#menu".into(),
            name: "menu".into(),
            selected_index: 0,
            bounds: BrowserBounds {
                x: 20.0,
                y: 40.0,
                width: 80.0,
                height: 22.0,
            },
            options: (0..10)
                .map(|index| NativeSelectOption {
                    index,
                    value: index.to_string(),
                    label: format!("Item {index}"),
                    group: None,
                    disabled: false,
                    selected: index == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn native_select_zoom_uses_current_menu_bounds_and_keeps_its_first_owner() {
        for fullscreen in [false, true] {
            for fixed in [false, true] {
                for native in [false, true] {
                    for starts_inside in [false, true] {
                        let event = if native {
                            egui::Event::Zoom(1.25)
                        } else {
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Line,
                                delta: egui::vec2(0.0, 4.0),
                                phase: egui::TouchPhase::Move,
                                modifiers: egui::Modifiers::CTRL,
                            }
                        };
                        check_select_zoom(fullscreen, fixed, &event, starts_inside);
                    }
                }
            }
        }
    }

    fn check_select_zoom(fullscreen: bool, fixed: bool, event: &egui::Event, starts_inside: bool) {
        use super::super::select_popup;
        use crate::{panel_zoom, test_egui::DiscardTextures};
        use egui::{Ui, vec2};
        use horizon_core::browser::BrowserPanelState;
        let native = matches!(event, egui::Event::Zoom(_));
        let popup = zoom_menu();
        let ctx = egui::Context::default();
        let browser = if fixed {
            BrowserPanelState::inert_remote("target", "provider")
        } else {
            BrowserPanelState::inert()
        };
        let mut state = BrowserUiState::default();
        let panel = egui::Id::new("browser-panel");
        let mut open = None;
        select_popup::sync_ui_state(&mut open, Some(&popup));
        for index in 0..5 {
            let active = index >= 3;
            // Chrome grows in the gesture's first frame. The old menu did not
            // cover y=360; the current layout does. Then the pointer crosses it.
            let header = if active { 90.0 } else { 0.0 };
            let inside = if index == 4 { !starts_inside } else { starts_inside };
            let pointer = pos2(if inside { 80.0 } else { 340.0 }, 360.0);
            let mut events = vec![egui::Event::PointerMoved(pointer)];
            if active {
                events.push(event.clone());
            }
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        time: Some(1.0 + f64::from(index) * 0.02),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let layer = if fullscreen { ui.layer_id().id } else { panel };
                        if active {
                            panel_zoom::gesture_owner(ui.ctx(), Some(panel_zoom::content_owner(layer, true)));
                        }
                        let mut draw = |ui: &mut Ui| {
                            ui.set_min_size(vec2(400.0, 500.0));
                            let menu = select_popup::show(
                                ui,
                                &browser,
                                Rect::from_min_size(pos2(0.0, header), vec2(400.0, 400.0)),
                                [400.0, 400.0],
                                &popup,
                                open.as_mut().expect("open menu"),
                            )
                            .expect("menu is rendered");
                            if !active {
                                return;
                            }
                            let hit = menu.contains(panel_zoom::local_pointer(ui).expect("pointer"));
                            assert_eq!(hit, inside);
                            let canvas = fixed && native && !fullscreen;
                            let dismiss = super::resolve_zoom_owner(ui, &browser, Some(menu.menu), fullscreen);
                            assert_eq!(dismiss, index == 3 && !starts_inside);
                            assert_eq!(panel_zoom::owns_gesture(ui), !starts_inside && !canvas);
                            let deferred = panel_zoom::take_deferred_canvas_zoom(ui.ctx());
                            assert_eq!(deferred.is_some(), index == 3 && !starts_inside && canvas);
                            if let Some(zoom) = deferred {
                                assert_eq!(zoom.anchor, pointer);
                                assert!((zoom.delta - 1.25).abs() < 0.001);
                            }
                            assert!(panel_zoom::take_deferred_canvas_zoom(ui.ctx()).is_none());
                            super::apply_zoom_gesture(ui, &browser, &mut state, panel_zoom::owns_gesture(ui));
                        };
                        if fullscreen {
                            draw(ui);
                        } else {
                            egui::Area::new(panel)
                                .fixed_pos(egui::Pos2::ZERO)
                                .constrain(false)
                                .order(egui::Order::Middle)
                                .show(ui.ctx(), draw);
                        }
                    },
                )
                .discard_textures();
        }
        assert_eq!(state.zoom.factor() > 1.0, !fixed && !starts_inside);
    }

    #[test]
    fn a_zoomed_viewport_stays_above_the_size_the_backend_sync_accepts() {
        use super::{MAX_FRAME_PIXELS, MIN_STABLE_VIEWPORT_SIDE, zoomed_viewport};
        let limit = 8192;
        // A roomy panel zooms exactly as asked.
        assert_eq!(zoomed_viewport(egui::vec2(800.0, 600.0), 4.0, limit), (200, 150));
        assert_eq!(zoomed_viewport(egui::vec2(800.0, 600.0), 0.5, limit), (1600, 1200));
        // Zooming out stops at the renderer's side limit and the pixel budget.
        let (wide, high) = zoomed_viewport(egui::vec2(3000.0, 2000.0), 0.25, limit);
        assert!(wide <= 8192 && high <= 8192, "{wide}x{high} passes the texture limit");
        assert!(
            u64::from(wide) * u64::from(high) <= u64::from(MAX_FRAME_PIXELS),
            "{wide}x{high} passes the pixel budget"
        );
        let (narrow, _) = zoomed_viewport(egui::vec2(3000.0, 200.0), 0.25, 2048);
        assert!(narrow <= 2048, "{narrow} passes a smaller renderer limit");
        // A short one caps the effective zoom instead of selecting a viewport
        // that would never be sent.
        // Conflicting constraints (a body far wider than tall) still produce a
        // viewport the backend sync will accept.
        let (long, short) = zoomed_viewport(egui::vec2(40000.0, 120.0), 4.0, limit);
        assert!(
            f32::from(u16::try_from(short).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE && long <= 8192,
            "{long}x{short} is not sendable"
        );
        let (width, height) = zoomed_viewport(egui::vec2(420.0, 100.0), 4.0, limit);
        assert!(
            f32::from(u16::try_from(height).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE
                && f32::from(u16::try_from(width).expect("small")) >= MIN_STABLE_VIEWPORT_SIDE,
            "{width}x{height} is below the stable viewport floor"
        );
    }

    #[test]
    fn rounded_viewports_stay_within_the_pixel_budget() {
        for size in [
            egui::vec2(254.0, 2041.0),
            egui::vec2(2041.0, 254.0),
            egui::vec2(960.0, 540.0),
            egui::vec2(3000.0, 2000.0),
        ] {
            for zoom in [0.25, 1.0, 4.0] {
                for side_limit in [2048, 4096, 16_384] {
                    let (width, height) = super::zoomed_viewport(size, zoom, side_limit);
                    assert!(width >= 33 && height >= 33);
                    let limit = u32::try_from(side_limit).expect("small texture limit");
                    assert!(width <= limit && height <= limit);
                    assert!(
                        u64::from(width) * u64::from(height) <= u64::from(super::MAX_FRAME_PIXELS),
                        "{size:?} at {zoom} with side limit {side_limit} produced {width}x{height}"
                    );
                }
            }
        }
        assert_eq!(
            super::zoomed_viewport(egui::vec2(960.0, 540.0), 0.25, 16_384),
            (3840, 2160),
            "a viewport exactly at the budget must remain unchanged"
        );
    }

    #[test]
    fn a_changed_effective_zoom_refreshes_the_idle_selector_then_settles() {
        use crate::{panel_zoom::PanelZoom, test_egui::DiscardTextures};
        use std::time::Duration;

        let ctx = egui::Context::default();
        let mut state = BrowserUiState {
            zoom: PanelZoom::new(0.25),
            ..Default::default()
        };
        let frame = |state: &mut BrowserUiState, available| {
            ctx.run_ui(egui::RawInput::default(), |ui| {
                crate::panel_zoom::dropdown(ui, "idle_zoom", &mut state.zoom, state.effective_zoom, true);
                super::sync_viewport_sizes(ui, state, available);
            })
            .discard_textures()
        };
        let repaint_delay = |output: &egui::FullOutput| {
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .expect("root viewport output")
                .repaint_delay
        };
        let mut available = egui::vec2(800.0, 600.0);
        for next in [egui::vec2(3000.0, 2000.0), available] {
            for _ in 0..8 {
                let _ = frame(&mut state, available);
            }
            assert!(
                !repaint_delay(&frame(&mut state, available)).is_zero(),
                "unchanged layout should idle"
            );
            let previous = state.effective_zoom;
            let changed = frame(&mut state, next);
            assert_ne!(state.effective_zoom, previous);
            assert_eq!(
                repaint_delay(&changed),
                Duration::ZERO,
                "the already-painted selector needs another frame"
            );

            let refreshed = frame(&mut state, next);
            let label = state.effective_zoom.label();
            assert!(
                refreshed.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == label)
                }),
                "the follow-up frame must paint the applied percentage"
            );
            available = next;
        }
        for _ in 0..8 {
            let _ = frame(&mut state, available);
        }
        assert!(
            !repaint_delay(&frame(&mut state, available)).is_zero(),
            "the final layout should idle"
        );
    }

    #[test]
    fn oversized_bodies_letterbox_at_the_reported_zoom_limit() {
        use crate::test_egui::DiscardTextures;
        use egui::vec2;
        let ctx = egui::Context::default();
        for available in [vec2(12_000.0, 12_000.0), vec2(40_000.0, 120.0)] {
            let mut state = BrowserUiState {
                zoom: crate::panel_zoom::PanelZoom::new(4.0),
                ..Default::default()
            };
            let _ = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    let (viewport, _) = super::sync_viewport_sizes(ui, &mut state, available);
                    let frame = vec2(
                        f32::from(u16::try_from(viewport.0).expect("bounded frame")),
                        f32::from(u16::try_from(viewport.1).expect("bounded frame")),
                    );
                    let scale = super::frame_scale(available, frame, true);
                    assert!(scale <= crate::panel_zoom::MAX_ZOOM);
                    assert!((scale - state.effective_zoom.factor()).abs() < 0.001);
                    assert!(u64::from(viewport.0) * u64::from(viewport.1) <= u64::from(super::MAX_FRAME_PIXELS));
                    assert!(frame.x * scale <= available.x && frame.y * scale <= available.y);
                })
                .discard_textures();
        }
        let fixed_scale = super::frame_scale(vec2(1000.0, 1000.0), vec2(100.0, 100.0), false);
        assert!((fixed_scale - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn host_dimensions_survive_responsive_frame_limits() {
        use crate::test_egui::DiscardTextures;
        let ctx = egui::Context::default();
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                for (size, expected) in [
                    (egui::vec2(12_000.4, 12_000.6), (12_000, 12_001)),
                    (egui::vec2(24.0, 30.0), (24, 30)),
                ] {
                    for zoom in [0.25, 1.0, 4.0] {
                        let mut state = BrowserUiState {
                            zoom: crate::panel_zoom::PanelZoom::new(zoom),
                            ..Default::default()
                        };
                        let (frame, host) = super::sync_viewport_sizes(ui, &mut state, size);
                        assert_eq!(host, expected);
                        assert_ne!(frame, host, "only the responsive frame should be capped");
                    }
                }
            })
            .discard_textures();
    }

    #[test]
    fn fixed_browser_keeps_its_zoom_preference_during_gestures() {
        use crate::test_egui::DiscardTextures;
        let ctx = egui::Context::default();
        let remote = horizon_core::browser::BrowserPanelState::inert_remote("target", "provider");
        let mut state = BrowserUiState {
            zoom: crate::panel_zoom::PanelZoom::new(1.5),
            ..Default::default()
        };
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Zoom(1.25)],
                    ..Default::default()
                },
                |ui| apply_zoom_gesture(ui, &remote, &mut state, true),
            )
            .discard_textures();
        assert_eq!(state.zoom, crate::panel_zoom::PanelZoom::new(1.5));
    }

    #[test]
    fn a_capped_selection_still_responds_to_the_first_gesture_back() {
        use crate::test_egui::DiscardTextures;
        let ctx = egui::Context::default();
        let gesture = |zoom: f32, effective: f32, delta: f32| {
            let mut state = BrowserUiState {
                zoom: crate::panel_zoom::PanelZoom::new(zoom),
                effective_zoom: crate::panel_zoom::PanelZoom::new(effective),
                ..BrowserUiState::default()
            };
            let output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Zoom(delta)],
                    ..Default::default()
                },
                |ui| apply_zoom_gesture(ui, &horizon_core::browser::BrowserPanelState::inert(), &mut state, true),
            );
            let _ = output.discard_textures();
            state.zoom.factor()
        };
        // A selection the panel had to raise to 85% zooms in from there, not
        // from the hidden 25%.
        assert!((gesture(0.25, 0.85, 1.2) - 1.02).abs() < 0.01);
        // Pushing further past the cap keeps walking the saved selection.
        assert!(
            (gesture(0.25, 0.85, 0.5) - 0.25).abs() < 0.01,
            "clamped at the range end"
        );
        // The same from the other side: a 400% selection capped to 130%.
        assert!((gesture(4.0, 1.3, 0.5) - 0.65).abs() < 0.01);
        assert!((gesture(4.0, 1.3, 1.5) - 4.0).abs() < 0.01, "clamped at the range end");
    }

    #[test]
    fn a_zoom_gesture_rescales_and_asks_for_the_frame_that_resends_the_viewport() {
        use crate::test_egui::DiscardTextures;
        let ctx = egui::Context::default();
        let zoom_input = || egui::RawInput {
            events: vec![egui::Event::Zoom(1.25)],
            ..Default::default()
        };
        let mut state = BrowserUiState::default();
        let mut repaint_requested = false;
        let output = ctx.run_ui(zoom_input(), |ui| {
            apply_zoom_gesture(ui, &horizon_core::browser::BrowserPanelState::inert(), &mut state, true);
            repaint_requested = ui.ctx().has_requested_repaint();
        });
        let _ = output.discard_textures();
        assert!((state.zoom.factor() - 1.25).abs() < 0.001);
        assert!(repaint_requested, "a static page needs the follow-up frame");

        // Off the body, and at the end of the range, nothing changes.
        let mut untouched = BrowserUiState::default();
        let output = ctx.run_ui(zoom_input(), |ui| {
            apply_zoom_gesture(
                ui,
                &horizon_core::browser::BrowserPanelState::inert(),
                &mut untouched,
                false,
            );
        });
        let _ = output.discard_textures();
        assert_eq!(untouched.zoom, crate::panel_zoom::PanelZoom::ONE);
        let mut clamped = BrowserUiState {
            zoom: crate::panel_zoom::PanelZoom::new(crate::panel_zoom::MAX_ZOOM),
            ..BrowserUiState::default()
        };
        let output = ctx.run_ui(zoom_input(), |ui| {
            apply_zoom_gesture(
                ui,
                &horizon_core::browser::BrowserPanelState::inert(),
                &mut clamped,
                true,
            );
        });
        let _ = output.discard_textures();
        assert!((clamped.zoom.factor() - crate::panel_zoom::MAX_ZOOM).abs() <= f32::EPSILON);
    }
}
