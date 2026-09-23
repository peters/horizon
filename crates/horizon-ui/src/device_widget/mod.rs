//! Native VNC rendering, read-only unless a person turns Interact on; agent
//! device actions stay in the CLI/MCP crate.
mod capture;
mod controls;
mod details;
mod frame;
mod host;
mod input;
mod observation;
mod session;

use egui::{ColorImage, TextureHandle, TextureOptions, Ui};
use horizon_core::{DevicePanelState, DeviceViewOptions, browser::manifest::device::DeviceServerDetails};

use frame::present_image;
use input::InputState;
use session::{DeviceRoute, Session, Status};

#[derive(Default)]
pub(crate) struct DeviceUiState {
    pub(crate) owner: Option<String>,
    pub(crate) host: host::HostState,
    image: ImageDisplay,
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
    desktop: Option<[usize; 2]>,
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
    pub(crate) fn was_rendered(&self) -> bool {
        self.rendered
    }

    pub(crate) fn begin_frame(&mut self) {
        self.host.begin_frame();
        self.image.previous_displayed = self.image.displayed;
        self.previous_rendered = self.rendered;
        self.image.displayed = false;
        self.rendered = false;
    }

    pub(crate) fn finish_frame(&mut self) {
        // A viewer that was not drawn this frame (hidden, collapsed, closed
        // workspace, another panel fullscreen) cannot see a release, so let go
        // of everything now rather than leave a key or button held remotely.
        if !self.rendered {
            self.release_input();
        }
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
            self.image.received_sequence = updates.received_frame_sequence;
            if let Some(desktop) = updates.desktop {
                self.desktop = Some(desktop);
                self.server.desktop_size = Some(desktop);
            }
            if let Some(name) = updates.server_name {
                self.server.name = Some(name);
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
        if details::header(ui, device, &self.server, &self.status, interactive, self.interact) {
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
        self.refresh_presentation(ui);
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
        self.image.last_uploaded = Some(std::time::Instant::now());
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
        self.connection_generation = self.connection_generation.saturating_add(1);
        self.image = ImageDisplay::default();
        self.server = DeviceServerDetails::default();
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
