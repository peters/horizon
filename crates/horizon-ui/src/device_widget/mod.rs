//! Read-only native VNC rendering; device actions stay in the CLI/MCP crate.
mod controls;
mod frame;
mod session;

use egui::{ColorImage, TextureHandle, TextureOptions, Ui};
use horizon_core::{
    DevicePanelState, DeviceViewOptions,
    browser::manifest::device::{Connection, ImageEvidence, PanelState},
};

use crate::panel_zoom::{self, PanelZoom};
use frame::present_image;
use session::{Session, Status};

#[derive(Default)]
pub(crate) struct DeviceUiState {
    pub(crate) owner: Option<String>,
    image: ImageDisplay,
    initialized: bool,
    rendered: bool,
    session: Option<Session>,
    /// Last full desktop from the worker. View controls crop/scale this locally.
    source: Option<ColorImage>,
    texture: Option<TextureHandle>,
    presented_options: Option<DeviceViewOptions>,
    /// Scroll offset a pointer-anchored zoom asks for on the next frame.
    pending_scroll: Option<egui::Vec2>,
    /// Anchor captured when the current gesture was claimed. egui keeps
    /// smoothing a gesture after the pointer has moved on, so recomputing the
    /// anchor from a pointer that has left the image would make it jump.
    zoom_anchor: Option<ZoomAnchor>,
    status: Status,
    desktop: Option<[usize; 2]>,
    controls: controls::Controls,
}

/// The pixel a gesture holds in place, in the panel's layer coordinates.
#[derive(Clone, Copy)]
struct ZoomAnchor {
    captured_at: f64,
    pointer: egui::Pos2,
    /// Source pixel under the pointer when the gesture started.
    content: egui::Vec2,
}

/// What `show_image` painted this frame, in the panel's layer coordinates.
#[derive(Clone, Copy)]
struct ImageView {
    scale: f32,
    image_rect: egui::Rect,
    body: egui::Rect,
}

#[derive(Default)]
struct ImageDisplay {
    sequence: u64,
    displayed: bool,
    previous_displayed: bool,
    /// This connection has uploaded a frame. A retained texture is not evidence.
    received: bool,
}

impl DeviceUiState {
    #[cfg(test)]
    pub(crate) fn was_rendered(&self) -> bool {
        self.rendered
    }

    pub(crate) fn begin_frame(&mut self) {
        self.image.previous_displayed = self.image.displayed;
        self.image.displayed = false;
        self.rendered = false;
    }

    pub(crate) fn finish_frame(&self) {
        if let Some(session) = &self.session {
            session.set_visible(self.rendered);
        }
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, device: &DevicePanelState, interactive: bool) {
        self.rendered = true;
        if !self.initialized {
            self.initialized = true;
            if device.connect_on_start {
                self.reconnect(ui.ctx(), device);
            }
        }
        let incoming = self.session.as_ref().map(|session| {
            let updates = session.take_updates(ui.ctx().viewport_id());
            let disconnected = matches!(updates.status, Some(Status::Disconnected(_) | Status::Stopped));
            let full = disconnected.then(|| session.latest_full()).flatten();
            (updates, full)
        });
        if let Some((updates, full)) = incoming {
            if let Some(desktop) = updates.desktop {
                self.desktop = Some(desktop);
            }
            if let Some(status) = updates.status {
                self.status = status;
            }
            if let Some(image) = updates.image {
                self.apply_worker_image(ui, image, updates.produced_with);
            }
            if let Some(full) = full {
                self.desktop = Some(full.size);
                self.source = Some(full);
                self.presented_options = None;
            }
        }
        if self.desktop.is_none()
            && let Some(source) = &self.source
        {
            self.desktop = Some(source.size);
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Read-only");
            ui.monospace(device.target.address().to_string());
            if ui.add_enabled(interactive, egui::Button::new("Reconnect")).clicked() {
                self.reconnect(ui.ctx(), device);
            }
            match &self.status {
                Status::Stopped => {
                    ui.label("Stopped — select Reconnect");
                }
                Status::Connecting => {
                    ui.spinner();
                    ui.label("Connecting…");
                }
                Status::Connected => {
                    ui.label("Connected");
                }
                Status::Disconnected(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, format!("Disconnected: {error}"));
                }
            }
            if self.controls.zoom_dropdown(ui, interactive) {
                // A chosen scale starts fresh: an offset or anchor computed for
                // the previous one would jump the image.
                self.pending_scroll = None;
                self.zoom_anchor = None;
            }
        });
        let previous = self.controls.options;
        let changed = ui
            .add_enabled_ui(interactive, |ui| {
                self.controls
                    .show(ui, self.desktop, self.texture.as_ref().map(TextureHandle::size))
            })
            .inner;
        if changed && self.apply_view_options(previous) {
            ui.ctx().request_repaint();
        }
        self.refresh_presentation(ui);
        ui.separator();
        if self.texture.is_none() {
            ui.label("The device desktop appears here after connection.");
        }
        let view = self.show_image(ui);
        if interactive {
            self.handle_zoom_gesture(ui, view);
        }
    }

    /// Paint the presented desktop at the selected scale. Returns what was
    /// painted, which a pointer-anchored zoom needs.
    fn show_image(&mut self, ui: &mut Ui) -> Option<ImageView> {
        let texture = self.texture.as_ref()?;
        let size = texture.size_vec2();
        let body = ui.available_rect_before_wrap();
        let available = body.size().max(egui::Vec2::ZERO);
        // Fit letterboxes the whole image; a zoom paints the presented pixels
        // at that scale and scrolls for whatever no longer fits.
        let scale = self
            .controls
            .zoom
            .map_or_else(|| (available.x / size.x).min(available.y / size.y), PanelZoom::factor);
        let (image_visible, image_rect) = if self.controls.zoom.is_some() {
            let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
            if let Some(offset) = self.pending_scroll.take() {
                area = area.scroll_offset(offset);
            }
            area.show(ui, |ui| centered_image(ui, texture, size * scale)).inner
        } else {
            self.pending_scroll = None;
            visible_image(ui, texture, size * scale)
        };
        self.image.displayed = image_visible && self.image.received && matches!(self.status, Status::Connected);
        Some(ImageView {
            scale,
            image_rect,
            body,
        })
    }

    /// Pinch, or wheel with the zoom modifier, anywhere over this panel. The
    /// scale still changes before any desktop has arrived; only the
    /// pointer-anchored scroll needs a painted image.
    fn handle_zoom_gesture(&mut self, ui: &Ui, view: Option<ImageView>) {
        let Some(delta) = panel_zoom::gesture_delta(ui, panel_zoom::owns_gesture(ui)) else {
            return;
        };
        // Start from the scale actually on screen, which for `Fit` can sit
        // outside the supported range.
        let displayed = view.map_or_else(|| self.controls.zoom.unwrap_or_default().factor(), |view| view.scale);
        let Some(next) = panel_zoom::gesture_target(displayed, delta) else {
            return;
        };
        self.anchor_zoom(ui, view, next);
        self.controls.zoom = Some(next);
        ui.ctx().request_repaint();
    }

    /// Hold one source pixel in place for the whole gesture. The anchor is
    /// captured where the gesture started and reused while it lasts, so the
    /// smoothed tail of a gesture whose pointer has left the image cannot
    /// drag the view somewhere else.
    fn anchor_zoom(&mut self, ui: &Ui, view: Option<ImageView>, next: PanelZoom) {
        let Some(view) = view else {
            return;
        };
        let now = ui.input(|input| input.time);
        let anchor = self
            .zoom_anchor
            .filter(|anchor| now - anchor.captured_at <= panel_zoom::GESTURE_IDLE_SECONDS)
            .or_else(|| {
                let pointer = panel_zoom::local_pointer(ui)?;
                // A degenerate layout has no image pixel to anchor on; the new
                // scale still applies.
                (view.scale.is_finite() && view.scale > 0.0 && view.body.contains(pointer)).then(|| ZoomAnchor {
                    captured_at: now,
                    pointer,
                    content: (pointer - view.image_rect.min) / view.scale,
                })
            });
        let Some(anchor) = anchor else {
            return;
        };
        self.pending_scroll =
            Some((anchor.content * next.factor() - (anchor.pointer - view.body.min)).max(egui::Vec2::ZERO));
        self.zoom_anchor = Some(ZoomAnchor {
            captured_at: now,
            ..anchor
        });
    }
    #[cfg(test)]
    fn set_source(&mut self, ui: &Ui, image: ColorImage) {
        self.desktop = Some(image.size);
        self.source = Some(image);
        self.presented_options = None;
        if self.refresh_presentation(ui) {
            self.record_received_frame();
        }
    }

    fn apply_worker_image(&mut self, ui: &Ui, image: ColorImage, produced_with: Option<DeviceViewOptions>) {
        let current = self.controls.options.for_desktop(self.desktop.unwrap_or(image.size));
        if produced_with.is_none_or(|produced| produced.same_presentation(current)) {
            let uploaded = self.upload_displayed(ui, image);
            self.presented_options = produced_with.or(Some(current));
            if uploaded {
                self.record_received_frame();
            }
            return;
        }
        // Newer pixels can arrive under older crop/limits. Re-present the retained
        // desktop so a later static period cannot keep the stale image.
        let Some(full) = self.session.as_ref().and_then(Session::latest_full) else {
            return;
        };
        self.desktop = Some(full.size);
        self.source = Some(full);
        self.presented_options = None;
        if self.refresh_presentation(ui) {
            self.record_received_frame();
        }
    }

    fn apply_view_options(&mut self, previous: DeviceViewOptions) -> bool {
        if let Some(session) = &self.session {
            session.set_options(self.controls.options);
        }
        if previous.same_presentation(self.controls.options) {
            return false;
        }
        if let Some(full) = self.session.as_ref().and_then(Session::latest_full) {
            self.source = Some(full);
        }
        self.presented_options = None;
        true
    }

    fn record_received_frame(&mut self) {
        self.image.received = true;
        self.image.sequence = self.image.sequence.saturating_add(1);
    }

    fn refresh_presentation(&mut self, ui: &Ui) -> bool {
        let Some(source) = self.source.as_ref() else {
            return false;
        };
        let options = self.controls.options.for_desktop(source.size);
        if self
            .presented_options
            .is_some_and(|presented| presented.same_presentation(options))
        {
            return false;
        }
        match present_image(source, options) {
            Ok(displayed) => {
                self.presented_options = Some(options);
                self.upload_displayed(ui, displayed)
            }
            Err(error) => {
                self.presented_options = Some(options);
                self.controls.set_error(error.to_string());
                false
            }
        }
    }

    fn upload_displayed(&mut self, ui: &Ui, image: ColorImage) -> bool {
        let limit = ui.ctx().input(|input| input.max_texture_side);
        if image.size.iter().any(|side| *side == 0 || *side > limit) {
            if let Some(full) = self.session.as_ref().and_then(Session::latest_full) {
                self.source = Some(full);
            }
            self.session = None;
            self.texture = None;
            self.status = Status::Disconnected("Desktop exceeds the renderer's texture limit".into());
            false
        } else if let Some(texture) = &mut self.texture {
            texture.set(image, TextureOptions::LINEAR);
            true
        } else {
            self.texture = Some(ui.ctx().load_texture("device-view", image, TextureOptions::LINEAR));
            true
        }
    }

    #[cfg(test)]
    fn update_texture(&mut self, ui: &Ui, image: ColorImage) {
        self.set_source(ui, image);
    }

    pub(crate) fn reconnect(&mut self, ctx: &egui::Context, device: &DevicePanelState) {
        self.initialized = true;
        self.image = ImageDisplay::default();
        if let Some(full) = self.session.as_ref().and_then(Session::latest_full) {
            self.source = Some(full);
        }
        self.session = None;
        match Session::start(
            device.target.address(),
            ctx.clone(),
            ctx.viewport_id(),
            self.controls.options,
        ) {
            Ok(session) => {
                self.session = Some(session);
                self.status = Status::Connecting;
            }
            Err(error) => self.status = Status::Disconnected(error.to_string()),
        }
    }

    pub(crate) fn observation(
        &mut self,
        panel_id: String,
        device: &DevicePanelState,
        visible: bool,
        actor: &str,
    ) -> PanelState {
        // Observe worker failures even when the panel is hidden or off canvas.
        // Retain pending pixels for show(); only rendering may assert display.
        if let Some(status) = self.session.as_ref().and_then(Session::take_status) {
            self.status = status;
        }
        let (connection, connection_error) = match &self.status {
            Status::Stopped => (Connection::Stopped, None),
            Status::Connecting => (Connection::Connecting, None),
            Status::Connected => (Connection::Connected, None),
            Status::Disconnected(error) => (Connection::Disconnected, Some(error.clone())),
        };
        PanelState {
            panel_id,
            endpoint: device.target.address().to_string(),
            visible,
            owned_by_caller: self.owner.as_deref() == Some(actor),
            image: ImageEvidence {
                image_received: self.image.received,
                image_displayed: visible
                    && self.image.received
                    && self.image.previous_displayed
                    && connection == Connection::Connected,
                frame_sequence: self.image.sequence,
            },
            connection,
            connection_error,
        }
    }
}

/// Center a scaled image that no longer fills its viewport. A zoom anchor is
/// only reachable while the image overflows — below that there is no scroll
/// range to hold a pixel in place — so the leftover space is split instead of
/// pinning the image to the scroll origin.
fn centered_image(ui: &mut Ui, texture: &TextureHandle, size: egui::Vec2) -> (bool, egui::Rect) {
    let slack = ((ui.available_size() - size) * 0.5).max(egui::Vec2::ZERO);
    if slack.y > 0.0 {
        ui.add_space(slack.y);
    }
    if slack.x <= 0.0 {
        return visible_image(ui, texture, size);
    }
    ui.horizontal(|ui| {
        ui.add_space(slack.x);
        visible_image(ui, texture, size)
    })
    .inner
}

/// Paint the presented image; report whether any of it survived clipping and
/// where it landed (the anchor for a pointer-centered zoom).
fn visible_image(ui: &mut Ui, texture: &TextureHandle, size: egui::Vec2) -> (bool, egui::Rect) {
    let response = ui.add(
        egui::Image::new(texture)
            .fit_to_exact_size(size)
            .sense(egui::Sense::hover()),
    );
    let painted = response.rect.intersect(ui.clip_rect());
    (painted.width() > 0.0 && painted.height() > 0.0, response.rect)
}

#[cfg(test)]
mod tests;
