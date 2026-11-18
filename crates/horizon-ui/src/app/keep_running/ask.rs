//! The ask that a close of the main window, or `Quit Horizon`, shows while
//! programs run.
//!
//! The ask draws what its counts say, and those counts came with the request. The
//! first action of a close is the one that Enter takes, the second is the
//! dangerous one, and a press outside the ask or the escape key cancels it.

use egui::{Context, Key, RichText};

use crate::theme;

use super::super::util::{ask_modal, chrome_button, danger_button, primary_button};
use super::{CloseAnswer, HorizonApp};

const CLOSE_HEADING: &str = "Close Horizon?";
const KEEP_RUNNING: &str = "Keep running in the background";
const KEEP_RUNNING_HELP: &str = "Start Horizon again to show the window. Menu, Quit Horizon ends it.";
const END_AND_QUIT: &str = "End them and quit";
const REMEMBER: &str = "Remember this choice";
const QUIT: &str = "Quit";
const CANCEL: &str = "Cancel";

impl HorizonApp {
    /// The ask of a close request or of `Quit Horizon`, and the answer of the
    /// person to it.
    pub(in crate::app) fn show_close_ask(&mut self, ctx: &Context) {
        let Some(ask) = self.keep_running.ask.as_ref() else {
            return;
        };
        let (running, from_menu) = (ask.running, ask.from_menu);
        let mut remember = ask.remember;
        let mut answer = None;
        let id = egui::Id::unique("close_horizon_ask");
        let shown = ask_modal(id).show(ctx, |ui| {
            ui.set_max_width(430.0);
            let heading = if from_menu {
                running.quit_question()
            } else {
                CLOSE_HEADING.to_owned()
            };
            ui.label(RichText::new(heading).size(13.0).color(theme::FG()));
            ui.add_space(6.0);
            for line in running
                .programs_line()
                .into_iter()
                .chain(running.outliving_lines(from_menu))
            {
                ui.label(RichText::new(line).size(12.5).color(theme::FG()));
            }
            ui.add_space(10.0);
            if from_menu {
                ui.horizontal(|ui| {
                    answer = actions(
                        ui,
                        &[
                            (QUIT, Look::Danger, CloseAnswer::Quit),
                            (CANCEL, Look::Plain, CloseAnswer::Cancel),
                        ],
                    );
                });
                return;
            }
            if action(ui, KEEP_RUNNING, Look::Primary) {
                answer = Some(CloseAnswer::KeepRunning);
            }
            ui.label(RichText::new(KEEP_RUNNING_HELP).size(11.5).color(theme::FG_SOFT()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                answer = actions(
                    ui,
                    &[
                        (END_AND_QUIT, Look::Danger, CloseAnswer::Quit),
                        (CANCEL, Look::Plain, CloseAnswer::Cancel),
                    ],
                );
            });
            // The first action is the one that Enter takes.
            if ui.input(|input| input.key_pressed(Key::Enter)) {
                answer.get_or_insert(CloseAnswer::KeepRunning);
            }
            ui.add_space(2.0);
            ui.checkbox(&mut remember, RichText::new(REMEMBER).size(12.0).color(theme::FG()));
        });
        if let Some(ask) = self.keep_running.ask.as_mut() {
            ask.remember = remember;
        }
        // A press outside the ask, or the escape key, is the `Cancel`.
        if let Some(answer) = answer.or_else(|| shown.should_close().then_some(CloseAnswer::Cancel)) {
            self.answer_close_ask(ctx, answer);
        }
    }
}

/// The look of an action of the ask, as the other questions of the window draw
/// theirs: the main action that Enter takes, the one that ends the programs, and
/// the plain one.
#[derive(Clone, Copy)]
enum Look {
    Primary,
    Danger,
    Plain,
}

/// Whether the person pressed the action `label`, drawn with `look`.
fn action(ui: &mut egui::Ui, label: &str, look: Look) -> bool {
    let button = match look {
        Look::Primary => primary_button(label),
        Look::Danger => danger_button(label),
        Look::Plain => chrome_button(label),
    };
    ui.add(button).clicked()
}

/// The action of the row of `entries` that the person pressed, if one.
fn actions(ui: &mut egui::Ui, entries: &[(&str, Look, CloseAnswer)]) -> Option<CloseAnswer> {
    let mut pressed = None;
    for (label, look, answer) in entries {
        if action(ui, label, *look) {
            pressed = Some(*answer);
        }
    }
    pressed
}
