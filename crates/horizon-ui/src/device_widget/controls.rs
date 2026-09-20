use egui::Ui;
use horizon_core::{DeviceViewOptions, DeviceViewport};

use crate::panel_zoom::{self, PanelZoom};

#[derive(Default)]
pub(super) struct Controls {
    pub options: DeviceViewOptions,
    /// Presented-image scale; `None` fits the whole image into the panel body.
    pub zoom: Option<PanelZoom>,
    pub(super) draft: Option<DeviceViewport>,
    error: Option<String>,
}

impl Controls {
    pub(super) fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Always-visible zoom selector, outside the collapsed view controls.
    /// Returns whether the selection changed.
    pub(super) fn zoom_dropdown(&mut self, ui: &mut Ui, interactive: bool) -> bool {
        panel_zoom::dropdown_with_fit(ui, "device_zoom", &mut self.zoom, interactive)
    }

    pub(super) fn show(&mut self, ui: &mut Ui, desktop: Option<[usize; 2]>, rendered: Option<[usize; 2]>) -> bool {
        let before = self.options;
        if let Some(desktop) = desktop {
            self.options = self.options.for_desktop(desktop);
            if self.options.viewport != before.viewport {
                self.draft = None;
                self.error = None;
            }
        }
        ui.collapsing("View controls", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Maximum fps");
                ui.add(egui::DragValue::new(&mut self.options.max_fps).range(1..=30));
                ui.label("Image limits");
                ui.add(
                    egui::DragValue::new(&mut self.options.max_width)
                        .range(1..=8192)
                        .suffix(" px wide"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.options.max_height)
                        .range(1..=8192)
                        .suffix(" px high"),
                );
            });
            ui.horizontal(|ui| {
                if (desktop.is_some() || self.options.viewport.is_some()) && ui.button("Whole desktop").clicked() {
                    self.options.viewport = None;
                    self.draft = None;
                    self.error = None;
                }
                if let (Some([source_width, source_height]), Some([image_width, image_height])) = (desktop, rendered) {
                    ui.label(format!(
                        "Desktop {source_width}×{source_height} · Image {image_width}×{image_height}"
                    ));
                }
            });
            if let Some(desktop) = desktop {
                let draft = self.draft.get_or_insert(DeviceViewport {
                    x: 0,
                    y: 0,
                    width: desktop[0],
                    height: desktop[1],
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Viewport");
                    ui.add(
                        egui::DragValue::new(&mut draft.x)
                            .range(0..=desktop[0].saturating_sub(1))
                            .prefix("x "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut draft.y)
                            .range(0..=desktop[1].saturating_sub(1))
                            .prefix("y "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut draft.width)
                            .range(1..=desktop[0])
                            .prefix("w "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut draft.height)
                            .range(1..=desktop[1])
                            .prefix("h "),
                    );
                    if ui.button("Apply viewport").clicked() {
                        let options = DeviceViewOptions {
                            viewport: Some(*draft),
                            ..self.options
                        };
                        match options.layout(desktop) {
                            Ok(_) => {
                                self.options = options;
                                self.error = None;
                            }
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                });
            }
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.weak("Image limits affect rendering; they do not change desktop resolution or VNC compression.");
        });
        before != self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn shrink_clears_an_outside_crop_and_its_draft() {
        let viewport = DeviceViewport {
            x: 1800,
            y: 900,
            width: 100,
            height: 100,
        };
        let mut controls = Controls {
            options: DeviceViewOptions {
                viewport: Some(viewport),
                ..Default::default()
            },
            draft: Some(viewport),
            ..Default::default()
        };
        let context = egui::Context::default();
        let _ = context
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(controls.show(ui, Some([1280, 720]), None));
            })
            .discard_textures();
        assert!(controls.options.viewport.is_none());
        assert!(controls.draft.is_none());
    }

    #[test]
    fn an_active_viewport_can_be_cleared_without_desktop_geometry() {
        let mut controls = Controls {
            options: DeviceViewOptions {
                viewport: Some(DeviceViewport {
                    x: 90,
                    y: 70,
                    width: 10,
                    height: 10,
                }),
                ..Default::default()
            },
            error: Some("desktop shrank".into()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut render = |events| {
            let mut changed = false;
            let output = ctx
                .run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        changed = controls.show(ui, None, None);
                    },
                )
                .discard_textures();
            (changed, output)
        };
        let text_center = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some(text.pos + text.galley.size() * 0.5),
                    _ => None,
                })
                .expect("control remains visible")
        };
        let (_, output) = render(Vec::new());
        let header = text_center(&output, "View controls");
        for pressed in [true, false] {
            render(vec![
                egui::Event::PointerMoved(header),
                egui::Event::PointerButton {
                    pos: header,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        let (_, output) = render(Vec::new());
        let button = text_center(&output, "Whole desktop");
        for pressed in [true, false] {
            let (changed, _) = render(vec![
                egui::Event::PointerMoved(button),
                egui::Event::PointerButton {
                    pos: button,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            if !pressed {
                assert!(changed);
            }
        }
        assert!(controls.options.viewport.is_none());
        assert!(controls.error.is_none());
    }
}
