use egui::{Label, Ui};
use horizon_vnc::{Region, ViewOptions};

use super::panel_text;
use crate::screen_pane;

#[derive(Default)]
pub(super) struct Controls {
    pub options: ViewOptions,
    pub one_to_one: bool,
    pub(super) draft: Option<Region>,
    error: Option<String>,
}

impl Controls {
    pub(super) fn set_error(&mut self, error: String) {
        self.error = Some(error);
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
        let heading = panel_text::heading(ui, "View controls");
        ui.collapsing(heading, |ui| {
            self.limits(ui);
            let whole_desktop = self.presentation(ui, desktop, rendered);
            if whole_desktop {
                self.options.viewport = None;
                self.draft = None;
                self.error = None;
            }
            if let Some(desktop) = desktop {
                self.viewport(ui, desktop);
            }
            self.notes(ui);
        });
        before != self.options
    }

    /// The rate and the size limits of the images that the host renders.
    pub(super) fn limits(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            row_of_numbers(ui);
            ui.add(Label::new(panel_text::word(
                ui,
                "Maximum fps",
                panel_text::SMALL,
                panel_text::fg(),
            )));
            screen_pane::add_widget(ui, ("max_fps",), |ui| {
                ui.add(egui::DragValue::new(&mut self.options.max_fps).range(1..=30))
            });
            ui.add(Label::new(panel_text::word(
                ui,
                "Image limits",
                panel_text::SMALL,
                panel_text::fg(),
            )));
            screen_pane::add_widget(ui, ("max_width",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.options.max_width)
                        .range(1..=8192)
                        .suffix(" px wide"),
                )
            });
            screen_pane::add_widget(ui, ("max_height",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.options.max_height)
                        .range(1..=8192)
                        .suffix(" px high"),
                )
            });
        });
    }

    /// The fit choice and the sizes of the desktop and of the image, as the
    /// answer to whether the person asked for the whole desktop.
    fn presentation(&mut self, ui: &mut Ui, desktop: Option<[usize; 2]>, rendered: Option<[usize; 2]>) -> bool {
        let mut whole_desktop = false;
        ui.horizontal(|ui| {
            let fit = panel_text::word(ui, "Fit", panel_text::SMALL, panel_text::fg());
            ui.selectable_value(&mut self.one_to_one, false, fit);
            let one_to_one = panel_text::word(ui, "1:1", panel_text::SMALL, panel_text::fg());
            ui.selectable_value(&mut self.one_to_one, true, one_to_one)
                .on_hover_text("One rendered image pixel per UI point; image limits still apply.");
            let whole = panel_text::word(ui, "Whole desktop", panel_text::SMALL, panel_text::fg());
            if (desktop.is_some() || self.options.viewport.is_some()) && ui.button(whole).clicked() {
                whole_desktop = true;
            }
            if let (Some([source_width, source_height]), Some([image_width, image_height])) = (desktop, rendered) {
                let limits = format!("Desktop {source_width}×{source_height} · Image {image_width}×{image_height}");
                let limits = panel_text::word(ui, &limits, panel_text::SMALL, panel_text::fg());
                ui.add(Label::new(limits));
            }
        });
        whole_desktop
    }

    /// The crop of the desktop that the host renders, with the word that applies
    /// it once the person is content with the four numbers.
    pub(super) fn viewport(&mut self, ui: &mut Ui, desktop: [usize; 2]) {
        let draft = self.draft.get_or_insert(Region {
            x: 0,
            y: 0,
            width: desktop[0],
            height: desktop[1],
        });
        ui.horizontal_wrapped(|ui| {
            row_of_numbers(ui);
            ui.add(Label::new(panel_text::word(
                ui,
                "Viewport",
                panel_text::SMALL,
                panel_text::fg(),
            )));
            screen_pane::add_widget(ui, ("viewport_x",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.x)
                        .range(0..=desktop[0].saturating_sub(1))
                        .prefix("x "),
                )
            });
            screen_pane::add_widget(ui, ("viewport_y",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.y)
                        .range(0..=desktop[1].saturating_sub(1))
                        .prefix("y "),
                )
            });
            screen_pane::add_widget(ui, ("viewport_w",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.width)
                        .range(1..=desktop[0])
                        .prefix("w "),
                )
            });
            screen_pane::add_widget(ui, ("viewport_h",), |ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.height)
                        .range(1..=desktop[1])
                        .prefix("h "),
                )
            });
            let apply = panel_text::word(ui, "Apply viewport", panel_text::SMALL, panel_text::fg());
            if ui.button(apply).clicked() {
                let options = ViewOptions {
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

    /// What the controls cannot do, and the last reason they refused a crop.
    fn notes(&self, ui: &mut Ui) {
        if let Some(error) = &self.error {
            let error = panel_text::wrapped(
                ui,
                error,
                panel_text::SMALL,
                ui.visuals().error_fg_color,
                ui.available_width(),
            );
            ui.add(Label::new(error));
        }
        let note = panel_text::wrapped(
            ui,
            "Image limits affect rendering; they do not change desktop resolution or VNC compression.",
            panel_text::SMALL,
            panel_text::weak(ui),
            ui.available_width(),
        );
        ui.add(Label::new(note));
    }
}

/// A row of a caption and numbers has the height of a number from its first
/// item. A wrapped row learns its height from the items that it has placed, so a
/// caption that comes first would otherwise stand at the top of the row and the
/// captions after it at the middle.
fn row_of_numbers(ui: &mut Ui) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let number = ui.fonts_mut(|fonts| fonts.row_height(&font)) + 2.0 * ui.spacing().button_padding.y;
    ui.set_row_height(number.max(ui.spacing().interact_size.y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    #[test]
    fn shrink_clears_an_outside_crop_and_its_draft() {
        let viewport = Region {
            x: 1800,
            y: 900,
            width: 100,
            height: 100,
        };
        let mut controls = Controls {
            options: ViewOptions {
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
            options: ViewOptions {
                viewport: Some(Region {
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
