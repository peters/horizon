//! Read-only native VNC rendering; device actions stay in the CLI/MCP crate.
mod controls;
mod frame;
mod session;

use egui::{TextureHandle, TextureOptions, Ui};
use horizon_core::DevicePanelState;

use session::{Session, Status};

#[derive(Default)]
pub(crate) struct DeviceUiState {
    initialized: bool,
    rendered: bool,
    session: Option<Session>,
    texture: Option<TextureHandle>,
    status: Status,
    desktop: Option<[usize; 2]>,
    controls: controls::Controls,
}

impl DeviceUiState {
    #[cfg(test)]
    pub(crate) fn was_rendered(&self) -> bool {
        self.rendered
    }

    pub(crate) fn begin_frame(&mut self) {
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
                self.connect(ui, device);
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
                self.connect(ui, device);
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
            if self.controls.one_to_one {
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(size)
                            .sense(egui::Sense::hover()),
                    );
                });
            } else {
                let available = ui.available_size().max(egui::Vec2::ZERO);
                let scale = (available.x / size.x).min(available.y / size.y);
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(size * scale)
                        .sense(egui::Sense::hover()),
                );
            }
        } else {
            ui.label("The device desktop appears here after connection.");
        }
    }

    fn update_texture(&mut self, ui: &Ui, image: egui::ColorImage) {
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

    fn connect(&mut self, ui: &Ui, device: &DevicePanelState) {
        self.session = None;
        self.texture = None;
        self.desktop = None;
        match Session::start(
            device.target.address(),
            ui.ctx().clone(),
            ui.ctx().viewport_id(),
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
}
