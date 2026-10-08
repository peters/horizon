//! Native VNC rendering, read-only unless a person turns Interact on; agent
//! device actions stay in the CLI/MCP crate.
mod capture;
mod controls;
mod details;
mod frame;
mod host;
mod input;
mod observation;
mod recording;
mod session;

use std::time::Duration;

use egui::{ColorImage, Context, TextureHandle, TextureOptions, Ui};
use horizon_core::{DevicePanelState, DeviceViewOptions, browser::manifest::device::DeviceServerDetails};

use frame::present_image;
use input::InputState;
use session::{DeviceRoute, Session, Status};

/// How often a connected viewer that is not drawn (off canvas, hidden, behind
/// a fullscreen panel) still uploads the latest received frame. Uploads keep
/// `frame_sequence` and the retained texture current for a person who comes
/// back and for agents reading evidence, without moving anyone's camera and
/// without repainting at the stream rate.
pub(super) const BACKGROUND_UPLOAD_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(crate) struct DeviceUiState {
    recording: recording::Recording,
    pub(crate) screenshots: crate::screenshot::Screenshots,
    pub(crate) owner: Option<String>,
    pub(crate) host: host::HostState,
    image: ImageDisplay,
    /// A real image was presented during this viewer's lifetime. Reconnecting
    /// resets transport evidence, but must not grant another canvas takeover.
    presented_once: bool,
    initialized: bool,
    rendered: bool,
    previous_rendered: bool,
    connection_generation: u64,
    session: Option<Session>,
    /// Last full desktop from the worker. View controls crop/scale this locally.
    source: Option<ColorImage>,
    texture: Option<TextureHandle>,
    presented_options: Option<DeviceViewOptions>,
    status: Status,
    server: DeviceServerDetails,
    native_labels: details::NativeLabels,
    desktop: Option<[usize; 2]>,
    /// What is shown is one flat colour: nothing is open on the desktop yet.
    empty_desktop: bool,
    controls: controls::Controls,
    /// A person's choice for this session only; never persisted, never set by agents.
    interact: bool,
    input: InputState,
    /// The image had keyboard focus last frame, so a loss must release keys.
    captured: bool,
    /// Where the pointer last was, in global coordinates, carried across
    /// frames so each wheel event is routed by the position it happened at.
    pointer_global: Option<egui::Pos2>,
}

#[derive(Default)]
struct ImageDisplay {
    sequence: u64,
    received_sequence: u64,
    displayed: bool,
    previous_displayed: bool,
    /// This connection has uploaded a frame. A retained texture is not evidence.
    received: bool,
    last_uploaded: Option<std::time::Instant>,
    last_displayed: Option<std::time::Instant>,
}

impl DeviceUiState {
    fn set_native_metadata(&mut self, metadata: horizon_core::browser::manifest::device::NativeSessionMetadata) {
        self.native_labels = details::NativeLabels::new(&metadata);
        self.server.native_session = Some(metadata);
    }

    pub(crate) fn screenshot_image(&mut self) -> Result<ColorImage, String> {
        if let Some(observation) = self.session.as_ref().map(Session::observation) {
            if let Some(status) = observation.status {
                self.status = status;
            }
            self.image.received_sequence = observation.received_frame_sequence;
        }
        if !matches!(self.status, Status::Connected) {
            return Err("Device viewer is not connected".into());
        }
        self.session
            .as_ref()
            .and_then(Session::latest_full)
            .ok_or_else(|| "Device viewer has not received a desktop frame".into())
    }

    /// Connected, and this session already has a desktop. Status becomes
    /// Connected before that frame, and copy or record cannot succeed until it arrives.
    fn has_current_desktop_frame(&self) -> bool {
        matches!(self.status, Status::Connected) && self.session.as_ref().is_some_and(Session::has_latest_full)
    }

    pub(crate) fn presented_once(&self) -> bool {
        self.presented_once
    }

    pub(crate) fn was_rendered(&self) -> bool {
        self.rendered
    }

    /// The last completed, non-discarded pass of this viewer's own viewport
    /// painted it after reveal `request` (or a later one) reached the canvas.
    pub(crate) fn displayed_since_reveal(&self, request: u64) -> bool {
        self.image.previous_displayed
            && !self.host.last_pass_discarded()
            && self
                .host
                .applied_at(request)
                .zip(self.image.last_displayed)
                .is_some_and(|(applied, displayed)| displayed >= applied)
    }

    pub(crate) fn begin_frame(&mut self) {
        self.host.begin_frame();
        self.commit_pass();
        self.image.displayed = false;
        self.rendered = false;
    }

    pub(crate) fn finish_frame(&mut self, ctx: &Context) {
        // A viewer that was not drawn this frame (hidden, collapsed, closed
        // workspace, another panel fullscreen) cannot see a release, so let go
        // of everything now rather than leave a key or button held remotely.
        if !self.rendered {
            self.release_input();
            self.upload_in_background(ctx);
        }
        if let Some(session) = &self.session {
            session.set_visible(self.rendered);
        }
        // Observations between passes (request pump, held reveals) describe
        // this completed pass rather than the one before it.
        self.commit_pass();
    }

    /// An undrawn viewer stays live: its evidence and texture follow the
    /// stream at a bounded cadence, independent of how often the host paints.
    fn upload_in_background(&mut self, ctx: &Context) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.pending_in_background(ctx.viewport_id()) {
            return;
        }
        let since_upload = self.image.last_uploaded.map(|uploaded| uploaded.elapsed());
        match since_upload {
            // A frame that arrived just after the last upload must not wait
            // for an unrelated repaint, or a stream that then goes static
            // would leave it unconsumed.
            Some(elapsed) if elapsed < BACKGROUND_UPLOAD_INTERVAL => {
                ctx.request_repaint_after(BACKGROUND_UPLOAD_INTERVAL.saturating_sub(elapsed));
            }
            _ => self.absorb_updates(ctx),
        }
    }

    fn absorb_updates(&mut self, ctx: &Context) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let updates = session.take_updates(ctx.viewport_id());
        let disconnected = matches!(updates.status, Some(Status::Disconnected(_) | Status::Stopped));
        let full = disconnected.then(|| session.latest_full()).flatten();
        self.image.received_sequence = updates.received_frame_sequence;
        if let Some(desktop) = updates.desktop {
            self.desktop = Some(desktop);
            self.server.desktop_size = Some(desktop);
        }
        if let Some(metadata) = updates.native_session {
            self.set_native_metadata(metadata);
        }
        if let Some(name) = updates.server_name {
            self.server.name = Some(name);
        }
        if let Some(status) = updates.status {
            self.status = status;
        }
        if let Some(image) = updates.image {
            self.apply_worker_image(ctx, image, updates.produced_with);
        }
        if let Some(full) = full {
            self.desktop = Some(full.size);
            self.source = Some(full);
            self.presented_options = None;
        }
    }

    /// A pass its viewport discarded was never presented, so it cannot be
    /// display evidence for inspection or a held reveal.
    fn commit_pass(&mut self) {
        self.image.previous_displayed = self.image.displayed && !self.host.last_pass_discarded();
        self.presented_once |= self.image.previous_displayed;
        self.previous_rendered = self.rendered;
    }

    pub(crate) fn show(&mut self, ui: &mut Ui, device: &DevicePanelState, interactive: bool) {
        self.rendered = true;
        if !self.initialized {
            self.initialized = true;
            if device.connect_on_start {
                self.reconnect(ui.ctx(), device);
            }
        }
        self.absorb_updates(ui.ctx());
        let mut screenshots = std::mem::take(&mut self.screenshots);
        ui.horizontal_wrapped(|ui| {
            screenshots.copy_button(ui, interactive && self.has_current_desktop_frame(), || {
                self.screenshot_image()
            });
            self.recording_controls(ui, interactive);
        });
        self.screenshots = screenshots;
        if self.desktop.is_none()
            && let Some(source) = &self.source
        {
            self.desktop = Some(source.size);
        }
        if details::header(
            ui,
            device,
            &self.server,
            &self.native_labels,
            &self.status,
            interactive,
            self.interact,
        ) {
            self.reconnect(ui.ctx(), device);
        }
        ui.add_enabled_ui(interactive, |ui| {
            details::show(ui, device, &self.server, matches!(self.status, Status::Connected));
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
        ui.add_enabled_ui(interactive, |ui| self.interact_toggle(ui));
        self.refresh_presentation(ui.ctx());
        ui.separator();
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let interact = self.interact && interactive;
            let (image_visible, response) = if self.controls.one_to_one {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| visible_image(ui, texture, size, interact))
                    .inner
            } else {
                let available = ui.available_size().max(egui::Vec2::ZERO);
                let scale = (available.x / size.x).min(available.y / size.y);
                visible_image(ui, texture, size * scale, interact)
            };
            if self.empty_desktop && image_visible && matches!(self.status, Status::Connected) {
                paint_empty_hint(ui, response.rect);
            }
            self.image.displayed = image_visible && self.image.received && matches!(self.status, Status::Connected);
            if self.image.displayed {
                self.image.last_displayed = Some(std::time::Instant::now());
            }
            // Only a visible image takes input: a clipped or off-canvas one
            // releases everything, as a hidden viewer does.
            if interact && image_visible && matches!(self.status, Status::Connected) {
                self.forward_input(ui, &response);
            } else {
                self.release_input();
            }
        } else {
            ui.label("The device desktop appears here after connection.");
        }
    }

    #[cfg(test)]
    fn set_source(&mut self, ctx: &Context, image: ColorImage) {
        self.desktop = Some(image.size);
        self.source = Some(image);
        self.presented_options = None;
        if self.refresh_presentation(ctx) {
            self.record_received_frame();
        }
    }

    fn apply_worker_image(&mut self, ctx: &Context, image: ColorImage, produced_with: Option<DeviceViewOptions>) {
        let current = self.controls.options.for_desktop(self.desktop.unwrap_or(image.size));
        if produced_with.is_none_or(|produced| produced.same_presentation(current)) {
            let uploaded = self.upload_displayed(ctx, image);
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
        if self.refresh_presentation(ctx) {
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
        self.image.last_uploaded = Some(std::time::Instant::now());
    }

    fn refresh_presentation(&mut self, ctx: &Context) -> bool {
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
                self.upload_displayed(ctx, displayed)
            }
            Err(error) => {
                self.presented_options = Some(options);
                self.controls.set_error(error.to_string());
                false
            }
        }
    }

    fn upload_displayed(&mut self, ctx: &Context, image: ColorImage) -> bool {
        self.empty_desktop = frame::looks_empty(&image);
        let limit = ctx.input(|input| input.max_texture_side);
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
            self.texture = Some(ctx.load_texture("device-view", image, TextureOptions::LINEAR));
            true
        }
    }

    /// A connected viewer holding one undelivered frame, without a VNC server.
    #[cfg(test)]
    pub(crate) fn connected_fixture(owner: &str) -> Self {
        let image = ColorImage::filled([4, 4], egui::Color32::GREEN);
        Self {
            owner: Some(owner.into()),
            initialized: true,
            status: Status::Connected,
            session: Some(Session::pending_frame(
                image.clone(),
                image,
                DeviceViewOptions::default(),
            )),
            ..Default::default()
        }
    }

    #[cfg(test)]
    fn update_texture(&mut self, ctx: &Context, image: ColorImage) {
        self.set_source(ctx, image);
    }

    pub(crate) fn reconnect(&mut self, ctx: &egui::Context, device: &DevicePanelState) {
        self.recording.stop();
        self.initialized = true;
        self.connection_generation = self.connection_generation.saturating_add(1);
        self.image = ImageDisplay::default();
        self.empty_desktop = false;
        self.server = DeviceServerDetails::default();
        self.native_labels = details::NativeLabels::default();
        self.input = InputState::default();
        self.captured = false;
        if let Some(full) = self.session.as_ref().and_then(Session::latest_full) {
            self.source = Some(full);
        }
        self.session = None;
        match Session::start(
            DeviceRoute::from(device),
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
}

const EMPTY_DESKTOP_HINT: &str = "Nothing is open on this desktop yet. Programs you start on it appear here.";

/// A blank picture reads as a broken connection, so the picture says what it is. Painted over
/// the part of it that is on screen and clipped to it, so nothing around the image moves or is
/// covered, and a large desktop scrolled in 1:1 mode still shows it.
fn paint_empty_hint(ui: &Ui, image: egui::Rect) {
    let visible = image.intersect(ui.clip_rect());
    if !visible.is_positive() {
        return;
    }
    let painter = ui.painter().with_clip_rect(visible);
    let color = egui::Color32::from_gray(210);
    let galley = painter.layout(
        EMPTY_DESKTOP_HINT.to_owned(),
        egui::FontId::proportional(14.0),
        color,
        (visible.width() - 32.0).max(40.0),
    );
    let backing = egui::Rect::from_center_size(visible.center(), galley.size() + egui::vec2(24.0, 14.0));
    painter.rect_filled(backing, 8.0, egui::Color32::from_black_alpha(170));
    painter.galley(backing.center() - galley.size() * 0.5, galley, color);
}

fn visible_image(ui: &mut Ui, texture: &TextureHandle, size: egui::Vec2, interact: bool) -> (bool, egui::Response) {
    let sense = if interact {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };
    let response = ui.add(egui::Image::new(texture).fit_to_exact_size(size).sense(sense));
    let visible = ui.is_rect_visible(response.rect) && response.rect.intersect(ui.clip_rect()).is_positive();
    (visible, response)
}

#[cfg(test)]
mod tests;
