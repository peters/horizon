//! Read-only native VNC rendering; device actions stay in the CLI/MCP crate.
mod controls;
mod frame;
mod session;

use egui::{ColorImage, TextureHandle, TextureOptions, Ui};
use horizon_core::{
    DevicePanelState, DeviceViewOptions,
    browser::manifest::device::{Connection, ImageEvidence, PanelState},
};

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
                let current = self.controls.options.for_desktop(self.desktop.unwrap_or(image.size));
                if updates.produced_with.is_none_or(|produced| produced == current) {
                    self.upload_displayed(ui, image);
                    self.presented_options = updates.produced_with.or(Some(current));
                    if self.texture.is_some() {
                        self.image.sequence = self.image.sequence.saturating_add(1);
                    }
                }
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
        });
        let changed = ui
            .add_enabled_ui(interactive, |ui| {
                self.controls
                    .show(ui, self.desktop, self.texture.as_ref().map(TextureHandle::size))
            })
            .inner;
        if changed {
            if let Some(session) = &self.session {
                session.set_options(self.controls.options);
                if let Some(full) = session.latest_full() {
                    self.source = Some(full);
                }
            }
            self.presented_options = None;
            ui.ctx().request_repaint();
        }
        self.refresh_presentation(ui);
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

    #[cfg(test)]
    fn set_source(&mut self, ui: &Ui, image: ColorImage) {
        self.desktop = Some(image.size);
        self.source = Some(image);
        self.presented_options = None;
        self.refresh_presentation(ui);
        if self.texture.is_some() {
            self.image.sequence = self.image.sequence.saturating_add(1);
        }
    }

    fn refresh_presentation(&mut self, ui: &Ui) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let options = self.controls.options.for_desktop(source.size);
        if self.presented_options == Some(options) {
            return;
        }
        match present_image(source, options) {
            Ok(displayed) => {
                self.presented_options = Some(options);
                self.upload_displayed(ui, displayed);
            }
            Err(error) => {
                self.presented_options = Some(options);
                self.controls.set_error(error.to_string());
            }
        }
    }

    fn upload_displayed(&mut self, ui: &Ui, image: ColorImage) {
        let limit = ui.ctx().input(|input| input.max_texture_side);
        if image.size.iter().any(|side| *side == 0 || *side > limit) {
            if let Some(full) = self.session.as_ref().and_then(Session::latest_full) {
                self.source = Some(full);
            }
            self.session = None;
            self.texture = None;
            self.status = Status::Disconnected("Desktop exceeds the renderer's texture limit".into());
        } else if let Some(texture) = &mut self.texture {
            texture.set(image, TextureOptions::LINEAR);
        } else {
            self.texture = Some(ui.ctx().load_texture("device-view", image, TextureOptions::LINEAR));
        }
    }

    #[cfg(test)]
    fn update_texture(&mut self, ui: &Ui, image: ColorImage) {
        self.set_source(ui, image);
    }

    pub(crate) fn reconnect(&mut self, ctx: &egui::Context, device: &DevicePanelState) {
        self.initialized = true;
        self.image.displayed = false;
        self.image.previous_displayed = false;
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

    fn fixture_device() -> DevicePanelState {
        DevicePanelState {
            target: horizon_core::DeviceViewTarget::parse("127.0.0.1:5900").unwrap(),
            connect_on_start: false,
        }
    }

    fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos + text.galley.size() * 0.5),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing label {label}"))
    }

    fn click_events(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn patterned_desktop() -> ColorImage {
        ColorImage::new(
            [8, 4],
            (0..32).map(|value| egui::Color32::from_rgb(value, 40, 80)).collect(),
        )
    }

    fn disconnected_viewer() -> (egui::Context, DevicePanelState, DeviceUiState) {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let state = DeviceUiState {
            initialized: true,
            status: Status::Disconnected("The VNC client isn't started. Or it is already closed".into()),
            ..Default::default()
        };
        (ctx, fixture_device(), state)
    }

    fn show_viewer(
        ctx: &egui::Context,
        state: &mut DeviceUiState,
        device: &DevicePanelState,
        events: Vec<egui::Event>,
    ) {
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    ..Default::default()
                },
                |ui| {
                    if state.source.is_none() {
                        state.update_texture(ui, patterned_desktop());
                    }
                    state.show(ui, device, true);
                },
            )
            .discard_textures();
    }

    fn click_label(ctx: &egui::Context, state: &mut DeviceUiState, device: &DevicePanelState, label: &str) {
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    ..Default::default()
                },
                |ui| state.show(ui, device, true),
            )
            .discard_textures();
        let pos = text_center(&output, label);
        for pressed in [true, false] {
            show_viewer(ctx, state, device, click_events(pos, pressed));
        }
    }

    fn presented_size(state: &DeviceUiState) -> Option<[usize; 2]> {
        state.texture.as_ref().map(TextureHandle::size)
    }

    #[test]
    fn narrow_frame_exceeding_gpu_limit_is_rejected_before_texture_upload() {
        let mut state = DeviceUiState::default();
        state.controls.options.max_width = 8192;
        state.controls.options.max_height = 8192;
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
        assert_eq!(state.image.sequence, 0, "rejected images are not uploaded frames");
    }

    #[test]
    fn image_limits_and_viewport_apply_without_a_live_session() {
        let (ctx, device, mut state) = disconnected_viewer();
        show_viewer(&ctx, &mut state, &device, Vec::new());
        assert_eq!(presented_size(&state), Some([8, 4]));
        assert_eq!(state.image.sequence, 1);

        state.controls.options.max_width = 4;
        state.controls.options.max_height = 4;
        state.presented_options = None;
        show_viewer(&ctx, &mut state, &device, Vec::new());
        assert_eq!(presented_size(&state), Some([4, 2]));
        assert_eq!(
            state.image.sequence, 1,
            "local presentation is not a new received frame"
        );

        state.controls.options.max_width = 2048;
        state.controls.options.max_height = 2048;
        state.controls.options.viewport = Some(horizon_core::DeviceViewport {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        });
        state.presented_options = None;
        show_viewer(&ctx, &mut state, &device, Vec::new());
        assert_eq!(presented_size(&state), Some([4, 4]));
    }

    #[test]
    fn desktop_shrink_lets_controls_clear_the_stale_viewport_draft() {
        let (ctx, device, mut state) = disconnected_viewer();
        let crop = horizon_core::DeviceViewport {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        };
        state.controls.options.viewport = Some(crop);
        state.controls.draft = Some(crop);
        show_viewer(&ctx, &mut state, &device, Vec::new());
        click_label(&ctx, &mut state, &device, "View controls");
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                    ..Default::default()
                },
                |ui| {
                    state.update_texture(ui, ColorImage::filled([4, 2], egui::Color32::WHITE));
                    state.show(ui, &device, true);
                },
            )
            .discard_textures();
        assert!(state.controls.options.viewport.is_none());
        assert_eq!(
            state.controls.draft.map(|draft| [draft.width, draft.height]),
            Some([4, 2])
        );
    }

    #[test]
    fn fit_one_to_one_and_whole_desktop_clicks_work_without_a_session() {
        let (ctx, device, mut state) = disconnected_viewer();
        state.controls.options.viewport = Some(horizon_core::DeviceViewport {
            x: 4,
            y: 0,
            width: 4,
            height: 4,
        });
        show_viewer(&ctx, &mut state, &device, Vec::new());
        assert_eq!(presented_size(&state), Some([4, 4]));
        click_label(&ctx, &mut state, &device, "View controls");
        click_label(&ctx, &mut state, &device, "1:1");
        assert!(state.controls.one_to_one);
        click_label(&ctx, &mut state, &device, "Fit");
        assert!(!state.controls.one_to_one);
        click_label(&ctx, &mut state, &device, "Whole desktop");
        assert!(state.controls.options.viewport.is_none());
        assert_eq!(presented_size(&state), Some([8, 4]));
    }

    #[test]
    fn apply_viewport_and_reconnect_keep_the_last_desktop() {
        let (ctx, device, mut state) = disconnected_viewer();
        show_viewer(&ctx, &mut state, &device, Vec::new());
        click_label(&ctx, &mut state, &device, "View controls");
        state.controls.draft = Some(horizon_core::DeviceViewport {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        });
        click_label(&ctx, &mut state, &device, "Apply viewport");
        assert_eq!(
            state
                .controls
                .options
                .viewport
                .map(|viewport| [viewport.width, viewport.height]),
            Some([2, 2])
        );
        assert_eq!(presented_size(&state), Some([2, 2]));
        click_label(&ctx, &mut state, &device, "Reconnect");
        assert!(matches!(state.status, Status::Connecting));
        assert_eq!(presented_size(&state), Some([2, 2]));
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
