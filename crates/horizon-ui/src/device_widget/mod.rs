//! Read-only native VNC rendering; device actions stay in the CLI/MCP crate.
mod controls;
mod frame;
mod session;

use egui::{TextureHandle, TextureOptions, Ui};
use horizon_core::DevicePanelState;
use horizon_core::browser::manifest::device::{Connection, ImageEvidence, PanelState};

use session::{Session, Status};

#[derive(Default)]
pub(crate) struct DeviceUiState {
    pub(crate) owner: Option<String>,
    image: ImageDisplay,
    initialized: bool,
    rendered: bool,
    session: Option<Session>,
    texture: Option<TextureHandle>,
    status: Status,
    desktop: Option<[usize; 2]>,
    controls: controls::Controls,
}

#[derive(Default)]
struct ImageDisplay {
    sequence: u64,
    displayed: bool,
    previous_displayed: bool,
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
        if let Some(session) = &self.session {
            let updates = session.take_updates(ui.ctx().viewport_id());
            self.desktop = updates.desktop;
            if let Some(status) = updates.status {
                self.status = status;
            }
            if let Some(image) = updates.image {
                self.update_texture(ui, image);
            }
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
        });
        let changed = ui
            .add_enabled_ui(interactive, |ui| {
                self.controls
                    .show(ui, self.desktop, self.texture.as_ref().map(TextureHandle::size))
            })
            .inner;
        if changed && let Some(session) = &self.session {
            session.set_options(self.controls.options);
        }
        ui.separator();
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let image_visible = if self.controls.one_to_one {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| visible_image(ui, texture, size))
                    .inner
            } else {
                let available = ui.available_size().max(egui::Vec2::ZERO);
                let scale = (available.x / size.x).min(available.y / size.y);
                visible_image(ui, texture, size * scale)
            };
            self.image.displayed = image_visible && matches!(self.status, Status::Connected);
        } else {
            ui.label("The device desktop appears here after connection.");
        }
    }

    fn update_texture(&mut self, ui: &Ui, image: egui::ColorImage) {
        self.image.sequence = self.image.sequence.saturating_add(1);
        let limit = ui.ctx().input(|input| input.max_texture_side);
        if image.size.iter().any(|side| *side == 0 || *side > limit) {
            self.session = None;
            self.texture = None;
            self.status = Status::Disconnected("Desktop exceeds the renderer's texture limit".into());
        } else if let Some(texture) = &mut self.texture {
            texture.set(image, TextureOptions::LINEAR);
        } else {
            self.texture = Some(ui.ctx().load_texture("device-view", image, TextureOptions::LINEAR));
        }
    }

    pub(crate) fn reconnect(&mut self, ctx: &egui::Context, device: &DevicePanelState) {
        self.initialized = true;
        self.image.sequence = 0;
        self.image.displayed = false;
        self.image.previous_displayed = false;
        self.session = None;
        self.texture = None;
        self.desktop = None;
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
                image_received: self.texture.is_some(),
                image_displayed: visible && self.image.previous_displayed && connection == Connection::Connected,
                frame_sequence: self.image.sequence,
            },
            connection,
            connection_error,
        }
    }
}

fn visible_image(ui: &mut Ui, texture: &TextureHandle, size: egui::Vec2) -> bool {
    let response = ui.add(
        egui::Image::new(texture)
            .fit_to_exact_size(size)
            .sense(egui::Sense::hover()),
    );
    let painted = response.rect.intersect(ui.clip_rect());
    painted.width() > 0.0 && painted.height() > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn narrow_frame_exceeding_gpu_limit_is_rejected_before_texture_upload() {
        let mut state = DeviceUiState::default();
        let ctx = egui::Context::default();
        let output = ctx.run_ui(
            egui::RawInput {
                max_texture_side: Some(2048),
                ..Default::default()
            },
            |ui| {
                state.update_texture(ui, egui::ColorImage::filled([2049, 1], egui::Color32::BLACK));
            },
        );
        let _ = output.discard_textures();
        assert!(state.texture.is_none());
        assert!(matches!(state.status, Status::Disconnected(_)));
    }
    #[test]
    fn connected_texture_is_not_display_proof_when_image_is_clipped() {
        let ctx = egui::Context::default();
        let device = DevicePanelState {
            target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
            connect_on_start: false,
        };
        for one_to_one in [false, true] {
            for (clip_height, expected) in [(20.0, false), (600.0, true)] {
                let mut state = DeviceUiState {
                    initialized: true,
                    status: Status::Connected,
                    ..Default::default()
                };
                state.controls.one_to_one = one_to_one;
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                        ..Default::default()
                    },
                    |ui| {
                        state.update_texture(ui, egui::ColorImage::filled([100, 100], egui::Color32::WHITE));
                        ui.set_clip_rect(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800.0, clip_height),
                        ));
                        state.show(ui, &device, false);
                    },
                );
                let _ = output.discard_textures();
                assert_eq!(state.image.displayed, expected, "1:1={one_to_one}, clip={clip_height}");
                state.begin_frame();
                assert_eq!(
                    state
                        .observation("panel".into(), &device, true, "agent")
                        .image
                        .image_displayed,
                    expected
                );
            }
        }
    }
}
