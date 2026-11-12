//! The frame source programs of the **Remote** tab.
//!
//! A native Device panel can show the pictures of a program of this computer. The
//! tab holds the list of those programs: a name for the person and for an agent, and
//! the one command line that starts it. Horizon holds no codec of such a program, so
//! this list is the whole of what it knows about one.
//!
//! Every row is two lines: the name with the button that removes it, and the command
//! line under it. Two fields of a three-field row do not fit the 444 points of the
//! panel of the smallest window that this build opens. A row of a line that holds
//! `{target}` gets a third line with the target that the person types, and the
//! `Open` button of the row starts the program in a viewer of the workspace.

use egui::Ui;
use horizon_core::{Config, FrameSource, MAX_NAME_CHARS, MAX_SOURCES, TARGET_PLACEHOLDER};

use crate::theme;

/// What one pass of the tab changed.
#[derive(Clone, Copy, Default)]
pub(super) struct FrameSourcesOutcome {
    /// The list of the settings changed.
    pub(super) changed: bool,
}

/// One program that the person asked to open, with the target of its row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct OpenRequest {
    pub(in crate::app) name: String,
    pub(in crate::app) target: Option<String>,
}

/// The values of this tab that the settings of this computer do not hold: the target
/// that a person typed, the program that the person asked to open, and the sentence
/// that says why it opened nothing.
#[derive(Default)]
pub(in crate::app) struct State {
    /// The text of the target field of each program, by the name of the program.
    targets: Vec<(String, String)>,
    open: Option<OpenRequest>,
    /// The name of the program of the last press and the sentence of its refusal.
    refusal: Option<(String, String)>,
}

impl State {
    /// The text of the target field of one program.
    fn target(&mut self, name: &str) -> &mut String {
        if let Some(index) = self.targets.iter().position(|(held, _)| held == name) {
            return &mut self.targets[index].1;
        }
        self.targets.push((name.to_owned(), String::new()));
        let index = self.targets.len() - 1;
        &mut self.targets[index].1
    }

    /// The program that the person asked to open, once.
    pub(in crate::app) fn take_open_request(&mut self) -> Option<OpenRequest> {
        self.open.take()
    }

    /// A program opened: no row keeps a sentence of a refusal.
    pub(in crate::app) fn opened(&mut self, name: &str) {
        if self.refusal.as_ref().is_some_and(|(refused, _)| refused == name) {
            self.refusal = None;
        }
    }

    /// Say why a program opened nothing, under its own row.
    pub(in crate::app) fn refuse(&mut self, name: &str, sentence: String) {
        self.refusal = Some((name.to_owned(), sentence));
    }

    /// The sentence of the refusal of one program, when it is the one of its row.
    fn refusal_of(&self, name: &str) -> Option<&str> {
        self.refusal
            .as_ref()
            .filter(|(refused, _)| refused == name)
            .map(|(_, sentence)| sentence.as_str())
    }

    /// Keep the targets of the programs that the section still holds.
    fn retain(&mut self, sources: &[FrameSource]) {
        if self.targets.is_empty() {
            return;
        }
        self.targets
            .retain(|(name, _)| sources.iter().any(|source| source.name.trim() == name));
    }
}

pub(super) fn render(ui: &mut Ui, config: &mut Config, state: &mut State) -> FrameSourcesOutcome {
    let mut outcome = FrameSourcesOutcome::default();
    super::section_heading(ui, "Frame source programs");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "A native Device panel can show the pictures of one program of this computer, which gives them \
             itself: Horizon holds no screen capture and no codec of such a program. The command line starts \
             the program when a viewer opens it, and {target} is the text that an agent may put there.",
        );
        ui.add_space(8.0);
        let mut remove = None;
        let mut open = None;
        for (index, source) in config.frame_sources.sources.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label("Name");
                let name = ui.add(
                    egui::TextEdit::singleline(&mut source.name)
                        .desired_width(150.0)
                        .hint_text("a name of your own"),
                );
                if name.changed() {
                    outcome.changed = true;
                }
                ui.add_space(4.0);
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
            let command = ui.add(
                egui::TextEdit::singleline(&mut source.command)
                    .desired_width(ui.available_width())
                    .hint_text("the program and its arguments"),
            );
            if command.changed() {
                outcome.changed = true;
            }
            let name = source.name.trim().to_owned();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(source.is_set(), egui::Button::new("Open"))
                    .on_disabled_hover_text("A program opens when it has a name and a command line")
                    .clicked()
                {
                    let target = state.target(&name);
                    open = Some(OpenRequest {
                        name: name.clone(),
                        target: (!target.trim().is_empty()).then(|| target.trim().to_owned()),
                    });
                }
                if source.command.contains(TARGET_PLACEHOLDER) {
                    ui.label("target");
                    let target = state.target(&name).clone();
                    let mut text = target;
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .desired_width(160.0)
                            .hint_text("the text of {target}"),
                    );
                    if field.changed() {
                        *state.target(&name) = text;
                    }
                }
            });
            if let Some(sentence) = state.refusal_of(&name) {
                ui.add(egui::Label::new(egui::RichText::new(sentence).size(11.0).color(theme::PALETTE_RED())).wrap());
            }
            ui.add_space(6.0);
        }
        if let Some(request) = open {
            state.open = Some(request);
        }
        if let Some(index) = remove {
            config.frame_sources.sources.remove(index);
            outcome.changed = true;
            state.refusal = None;
        }
        state.retain(&config.frame_sources.sources);
        let full = config.frame_sources.sources.len() >= MAX_SOURCES;
        if ui
            .add_enabled(!full, egui::Button::new("Add a program"))
            .on_disabled_hover_text(format!(
                "This Horizon keeps at most {MAX_SOURCES} frame source programs"
            ))
            .clicked()
        {
            config.frame_sources.sources.push(horizon_core::FrameSource::default());
            outcome.changed = true;
        }
        if let Err(error) = config.frame_sources.validate() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(error).size(11.0).color(theme::FG_DIM()));
        }
        ui.add_space(2.0);
        super::dim_label(
            ui,
            &format!("A name is 1 to {MAX_NAME_CHARS} characters of A-Z, a-z, 0-9, '-' and '_'."),
        );
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::{OpenRequest, State, render};
    use crate::test_egui::DiscardTextures;
    use horizon_core::{Config, FrameSource};

    fn section(name: &str, command: &str) -> Config {
        let mut config = Config::default();
        config.frame_sources.sources.push(FrameSource {
            name: name.to_owned(),
            command: command.to_owned(),
        });
        config
    }

    /// One frame of the section, with the events of a person.
    fn frame(
        ctx: &egui::Context,
        config: &mut Config,
        state: &mut State,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 700.0))),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        render(ui, config, state);
                    });
                });
            },
        )
        .discard_textures()
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

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn press(ctx: &egui::Context, config: &mut Config, state: &mut State, at: egui::Pos2) {
        for pressed in [true, false] {
            frame(
                ctx,
                config,
                state,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }

    #[test]
    fn the_open_button_of_a_row_asks_for_that_program() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let mut config = section("bench", "/usr/bin/a-frame-source-program --size 640x360");
        let mut state = State::default();
        let output = frame(&ctx, &mut config, &mut state, Vec::new());
        let open = text_center(&output, "Open");
        press(&ctx, &mut config, &mut state, open);
        assert_eq!(
            state.take_open_request(),
            Some(OpenRequest {
                name: "bench".to_owned(),
                target: None
            }),
            "the row of the program asks for its own viewer"
        );
        assert!(state.take_open_request().is_none(), "one press is one viewer");
    }

    #[test]
    fn a_row_of_a_line_with_a_placeholder_has_a_target_field() {
        let ctx = egui::Context::default();
        let mut plain = section("bench", "/usr/bin/a-frame-source-program --size 640x360");
        let mut with_target = section("bench", "/usr/bin/a-frame-source-program {target}");
        let mut state = State::default();
        let plain_texts = texts(&frame(&ctx, &mut plain, &mut state, Vec::new()));
        let target_texts = texts(&frame(&ctx, &mut with_target, &mut state, Vec::new()));
        assert!(!plain_texts.iter().any(|text| text == "target"), "{plain_texts:?}");
        assert!(
            target_texts.iter().any(|text| text == "target"),
            "the line holds the placeholder, so the row asks for its text: {target_texts:?}"
        );
        assert!(
            target_texts.iter().any(|text| text == "Open"),
            "both rows open a viewer"
        );
    }

    #[test]
    fn a_program_without_a_name_or_a_command_opens_nothing() {
        let ctx = egui::Context::default();
        let mut config = Config::default();
        config.frame_sources.sources.push(FrameSource::default());
        let mut state = State::default();
        let output = frame(&ctx, &mut config, &mut state, Vec::new());
        let open = text_center(&output, "Open");
        press(&ctx, &mut config, &mut state, open);
        assert!(
            state.take_open_request().is_none(),
            "a program that is not set opens nothing"
        );
    }

    #[test]
    fn the_refusal_of_a_program_is_the_line_of_its_own_row() {
        let ctx = egui::Context::default();
        let mut config = section("bench", "/usr/bin/a-frame-source-program {target}");
        config.frame_sources.sources.push(FrameSource {
            name: "lab".to_owned(),
            command: "/usr/bin/a-frame-source-program".to_owned(),
        });
        let mut state = State::default();
        state.refuse(
            "bench",
            "the target of a frame source viewer is not one target".to_owned(),
        );
        let drawn = texts(&frame(&ctx, &mut config, &mut state, Vec::new()));
        assert!(
            drawn
                .iter()
                .any(|text| text == "the target of a frame source viewer is not one target"),
            "the sentence of the refusal is drawn beside its own row: {drawn:?}"
        );
    }
}
