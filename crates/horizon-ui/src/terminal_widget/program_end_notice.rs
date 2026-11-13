//! The line of a panel whose program ended.
//!
//! A shell, a command or an agent that ends leaves its last screen in the panel,
//! and the grid then takes no key: nothing reads them. The person reads here what
//! happened, which keys reach nothing, and where the panel starts again or closes.
//! Horizon never closes the panel and never starts the program again because it
//! ended; both are the person's choice.

use egui::{Rect, Ui};
use horizon_core::ProgramEnd;

use super::notice_strip;

/// What the person pressed in the line under the grid of one panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelNoticeAction {
    /// Nothing of this frame's pass touched a line.
    #[default]
    None,
    /// The next attempt of a machine terminal that lost its link.
    Reconnect,
    /// The program of the panel starts again in the same panel, with the
    /// command, the arguments, the folder and the environment of its launch.
    StartProgramAgain,
    /// The panel closes, as its close control closes it.
    ClosePanel,
}

/// What one panel says under its grid while it holds no program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProgramEndNotice {
    /// The end of the program, which holds the exit status of the sentence.
    pub(crate) end: ProgramEnd,
}

impl ProgramEndNotice {
    /// The line of `end`: what happened, then that typed text reaches nothing.
    pub(crate) fn line(self) -> String {
        self.end.line()
    }

    /// Paints the line under the grid of `body` and answers what the person
    /// pressed in it.
    pub(crate) fn show(self, ui: &mut Ui, body: Rect, interactive: bool) -> PanelNoticeAction {
        let Some(strip) = notice_strip::paint(ui, body) else {
            return PanelNoticeAction::None;
        };
        let line = self.line();
        notice_strip::row(ui, strip, |ui| {
            notice_strip::sides(ui, &line, |ui| {
                // The controls read from the right: Close sits at the edge, and
                // the control that keeps the panel comes before it.
                let close = notice_strip::control(ui, "Close", interactive);
                let start_again = notice_strip::control(ui, "Start again", interactive);
                match (start_again, close) {
                    (true, _) => PanelNoticeAction::StartProgramAgain,
                    (false, true) => PanelNoticeAction::ClosePanel,
                    (false, false) => PanelNoticeAction::None,
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, Vec2};
    use horizon_core::ProgramEnd;

    use super::{PanelNoticeAction, ProgramEndNotice};
    use crate::test_egui::{DiscardTextures, accesskit_labels};

    fn notice(status: Option<i32>) -> ProgramEndNotice {
        ProgramEndNotice {
            end: ProgramEnd { exit_status: status },
        }
    }

    fn body() -> Rect {
        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(520.0, 140.0))
    }

    /// One pass of the strip of `notice`, with `events` as its input.
    fn frame(
        ctx: &egui::Context,
        notice: ProgramEndNotice,
        interactive: bool,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, PanelNoticeAction) {
        let mut action = PanelNoticeAction::None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0))),
                    ..egui::RawInput::default()
                },
                |ui| {
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(body())
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                        |ui| {
                            action = notice.show(ui, body(), interactive);
                        },
                    );
                },
            )
            .discard_textures();
        (output, action)
    }

    /// The centre of the drawn text `label`, so a press reaches its control.
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

    fn press(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
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

    fn clicked(notice: ProgramEndNotice, label: &str) -> PanelNoticeAction {
        let ctx = egui::Context::default();
        let (output, _) = frame(&ctx, notice, true, Vec::new());
        let centre = text_center(&output, label);
        frame(&ctx, notice, true, press(centre, true));
        let (_, action) = frame(&ctx, notice, true, press(centre, false));
        action
    }

    /// C1: the line says what happened and that a key reaches nothing, in the
    /// form of the line of a panel that lost its link.
    #[test]
    fn the_line_says_the_program_ended_and_that_typed_text_is_not_sent() {
        assert_eq!(
            notice(None).line(),
            "The program of this panel ended. Text typed here is not sent."
        );
        assert_eq!(
            notice(Some(0)).line(),
            "The program of this panel ended. Text typed here is not sent."
        );
        assert_eq!(
            notice(Some(1)).line(),
            "The program of this panel ended with status 1. Text typed here is not sent."
        );
        assert_eq!(
            notice(Some(130)).line(),
            "The program of this panel ended with status 130. Text typed here is not sent."
        );
    }

    /// The text that one pass draws, in draw order.
    fn drawn_texts(interactive: bool) -> Vec<String> {
        let ctx = egui::Context::default();
        let (output, _) = frame(&ctx, notice(Some(2)), interactive, Vec::new());
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    /// C2: one line and two controls.
    #[test]
    fn the_line_offers_start_again_and_close() {
        // The controls are drawn first and the sentence last, because the right
        // side of the strip takes its room before the sentence fills the rest.
        assert_eq!(
            drawn_texts(true),
            vec![
                "Close".to_owned(),
                "Start again".to_owned(),
                "The program of this panel ended with status 2. Text typed here is not sent.".to_owned(),
            ]
        );
        let labels = accesskit_labels(|ui| {
            notice(Some(2)).show(ui, body(), true);
        });
        assert!(
            labels.iter().all(|(_, disabled)| !disabled),
            "an interactive panel offers both controls: {labels:?}"
        );
    }

    /// C3: each control asks for its own work, and the line asks for nothing by
    /// itself.
    #[test]
    fn a_press_on_a_control_asks_for_that_work_only() {
        assert_eq!(
            clicked(notice(None), "Start again"),
            PanelNoticeAction::StartProgramAgain
        );
        assert_eq!(clicked(notice(None), "Close"), PanelNoticeAction::ClosePanel);
        let (_, action) = frame(&egui::Context::default(), notice(None), true, Vec::new());
        assert_eq!(
            action,
            PanelNoticeAction::None,
            "a pass without a press asks for nothing"
        );
    }

    #[test]
    fn a_panel_that_cannot_be_pressed_still_says_its_line() {
        assert_eq!(drawn_texts(false).len(), 3, "the sentence and both controls stay");
        let labels = accesskit_labels(|ui| {
            notice(None).show(ui, body(), false);
        });
        assert_eq!(
            labels.iter().filter(|(_, disabled)| *disabled).count(),
            2,
            "a read-only panel shows both controls disabled: {labels:?}"
        );
    }

    /// C4: the line sits under the grid, so a body that is off the screen paints
    /// nothing and takes no press.
    #[test]
    fn a_body_that_is_off_the_screen_answers_nothing() {
        let ctx = egui::Context::default();
        let mut action = PanelNoticeAction::Reconnect;
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                    ..egui::RawInput::default()
                },
                |ui| {
                    let off = Rect::from_min_size(egui::pos2(-900.0, -900.0), Vec2::new(80.0, 60.0));
                    action = notice(None).show(ui, off, true);
                },
            )
            .discard_textures();
        assert_eq!(action, PanelNoticeAction::None, "an unseen strip takes no press");
    }
}
