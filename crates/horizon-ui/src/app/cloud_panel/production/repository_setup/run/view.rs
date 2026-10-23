//! The setup agent's progress view: a card with the steps, their elapsed times and
//! the actions, and the agent's screen in a log well when it has no column of its own.
use super::{Action, Fixer, Run, State, Step};
use crate::app::cloud_panel::runtime::{action_button, danger_button};
use crate::theme;
use egui::{Align, Color32, CornerRadius, Frame, Key, Layout, RichText, Sense, Stroke, TextEdit, Ui, Vec2};
use std::time::Duration;

/// Dialog buttons and the steering field follow the creation dialog's sizes.
const BUTTON: Vec2 = Vec2::new(120.0, 40.0);

/// The progress view in place of the setup choices: the steps and the steering
/// row in a card, then the agent's own screen in a well that fills the height left.
/// `output_height` places the output well below the steps; `None` leaves it to a
/// column of its own.
pub(in crate::app::cloud_panel::production::repository_setup) fn render(
    ui: &mut Ui,
    run: &mut Run,
    repository: &str,
    output_height: Option<f32>,
) -> Option<Action> {
    run.poll(&ui.ctx().clone());
    let mut action = None;
    let steps = run.steps();
    let finished = run.finished();
    let working = !finished && !run.stopped;
    if working {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
    let tint = if finished {
        theme::PALETTE_GREEN()
    } else {
        theme::ACCENT()
    };
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG_ALT(), tint, 0.06))
        .stroke(Stroke::new(1.0, theme::blend(theme::BORDER_SUBTLE(), tint, 0.32)))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            header(ui, run, repository, finished);
            ui.add_space(10.0);
            progress_bar(ui, &steps, tint);
            ui.add_space(10.0);
            for step in &steps {
                step_row(ui, step, run.elapsed(&step.title));
            }
            if let Some(proof) = run
                .proof
                .as_ref()
                .filter(|proof| proof.running() || !proof.log.is_empty())
            {
                ui.add_space(8.0);
                proof_tail(ui, proof);
            }
            if working && let Some(fix) = run.proof.as_ref().and_then(|proof| proof.fix.clone()) {
                ui.add_space(8.0);
                action = fix_card(ui, &fix, run.fixer);
            }
            if let Some(question) = run.status.question.as_deref().filter(|_| working) {
                ui.add_space(8.0);
                callout(ui, question);
            }
            if let Some(notice) = &run.notice {
                ui.add_space(8.0);
                ui.colored_label(theme::PALETTE_RED(), notice);
            }
            ui.add_space(12.0);
            // Beside the agent's own terminal, the person types there; inline, a field
            // passes messages and the footer is not the only place to act.
            let inline = output_height.is_some();
            let pressed = if finished {
                finished_actions(ui, run, inline)
            } else {
                working_actions(ui, run, working, inline)
            };
            action = action.take().or(pressed);
        });
    if let Some(height) = output_height {
        ui.add_space(12.0);
        output_well(ui, run, height);
    }
    action
}

fn header(ui: &mut Ui, run: &Run, repository: &str, finished: bool) {
    ui.horizontal(|ui| {
        if finished {
            badge(ui, theme::PALETTE_GREEN(), "✓");
        } else if run.stopped {
            badge(ui, theme::PALETTE_RED(), "✕");
        } else {
            ui.add(egui::Spinner::new().size(18.0).color(theme::ACCENT()));
        }
        let title = if finished {
            "Ready".to_owned()
        } else {
            format!("Setting up {repository}")
        };
        ui.label(RichText::new(title).size(17.0).strong().color(theme::FG()));
        chip(ui, &run.agent);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(clock(run.started.elapsed()))
                    .monospace()
                    .size(13.0)
                    .color(theme::FG_SOFT()),
            );
        });
    });
    if let Some(summary) = run.status.summary.as_deref() {
        ui.add_space(4.0);
        ui.label(RichText::new(summary).size(14.0).color(theme::FG_SOFT()));
    }
}

fn progress_bar(ui: &mut Ui, steps: &[Step], tint: Color32) {
    let complete = steps
        .iter()
        .filter(|step| matches!(step.state, State::Done | State::Skipped))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let fraction = if steps.is_empty() {
        0.0
    } else {
        complete as f32 / steps.len() as f32
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), theme::alpha(theme::BORDER_SUBTLE(), 200));
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.max(0.03));
    painter.rect_filled(filled, CornerRadius::same(2), tint);
}

fn step_row(ui: &mut Ui, step: &Step, elapsed: Option<Duration>) {
    ui.horizontal(|ui| {
        ui.set_min_height(28.0);
        let (icon, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        step_icon(ui, icon, step.state);
        // The painted icon tells a screen reader the step's state, as the deployment's do.
        let state = match step.state {
            State::Pending => "pending",
            State::Running => "running",
            State::Done => "done",
            State::Skipped => "skipped",
            State::Failed => "failed",
        };
        let spoken = match elapsed {
            Some(elapsed) => format!("{}: {state}, {} s", step.title, elapsed.as_secs()),
            None => format!("{}: {state}", step.title),
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, spoken.clone()));
        ui.add_space(6.0);
        let color = match step.state {
            State::Running => theme::FG(),
            State::Pending => theme::FG_DIM(),
            State::Failed => theme::PALETTE_RED(),
            State::Done | State::Skipped => theme::FG_SOFT(),
        };
        let title = RichText::new(&step.title).size(14.0).color(color);
        let title = if step.state == State::Running {
            title.strong()
        } else {
            title
        };
        ui.allocate_ui_with_layout(Vec2::new(150.0, 20.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(150.0);
            ui.label(title);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let time = elapsed.filter(|_| step.state != State::Pending).map(clock);
            ui.add_sized(
                Vec2::new(52.0, 20.0),
                egui::Label::new(RichText::new(time.unwrap_or_default()).monospace().size(12.5).color(
                    if step.state == State::Running {
                        theme::ACCENT()
                    } else {
                        theme::FG_SOFT()
                    },
                )),
            );
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                if !step.detail.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&step.detail).size(12.5).color(theme::FG_SOFT())).truncate());
                }
            });
        });
    });
}

/// Minutes and seconds, as the dialog shows every elapsed time.
fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn step_icon(ui: &Ui, rect: egui::Rect, state: State) {
    let painter = ui.painter();
    let center = rect.center();
    match state {
        State::Done => {
            painter.circle_filled(center, 8.0, theme::alpha(theme::PALETTE_GREEN(), 60));
            painter.circle_stroke(center, 8.0, Stroke::new(1.5, theme::PALETTE_GREEN()));
            let check = [
                center + Vec2::new(-3.5, 0.2),
                center + Vec2::new(-1.0, 2.8),
                center + Vec2::new(3.8, -2.6),
            ];
            painter.line(check.to_vec(), Stroke::new(1.8, theme::PALETTE_GREEN()));
        }
        State::Running => {
            let time = ui.input(|input| input.time);
            #[allow(clippy::cast_possible_truncation)]
            let start = (time * 4.0) as f32;
            let points = (0..=20)
                .map(|index| {
                    #[allow(clippy::cast_precision_loss)]
                    let angle = start + index as f32 / 20.0 * 4.4;
                    center + Vec2::angled(angle) * 7.5
                })
                .collect();
            painter.circle_stroke(center, 7.5, Stroke::new(1.5, theme::alpha(theme::ACCENT(), 50)));
            painter.line(points, Stroke::new(2.0, theme::ACCENT()));
            ui.ctx().request_repaint();
        }
        State::Pending => {
            painter.circle_stroke(center, 7.0, Stroke::new(1.2, theme::alpha(theme::FG_DIM(), 140)));
        }
        State::Skipped => {
            painter.circle_stroke(center, 7.0, Stroke::new(1.2, theme::alpha(theme::FG_DIM(), 140)));
            painter.line_segment(
                [center - Vec2::new(3.5, 0.0), center + Vec2::new(3.5, 0.0)],
                Stroke::new(1.5, theme::FG_DIM()),
            );
        }
        State::Failed => {
            painter.circle_stroke(center, 8.0, Stroke::new(1.5, theme::PALETTE_RED()));
            let arm = Vec2::splat(3.0);
            let stroke = Stroke::new(1.8, theme::PALETTE_RED());
            painter.line_segment([center - arm, center + arm], stroke);
            painter.line_segment([center + Vec2::new(-3.0, 3.0), center + Vec2::new(3.0, -3.0)], stroke);
        }
    }
}

/// The last lines of Horizon's own proof run, so a build or test can be followed.
fn proof_tail(ui: &mut Ui, proof: &super::Proof) {
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for line in proof.log.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                ui.add(egui::Label::new(RichText::new(line).monospace().size(11.5).color(theme::FG_SOFT())).truncate());
            }
        });
}

/// What Horizon recognised in a failed proof, and the person's ways to fix it: let the
/// agent do it in a browser panel they can watch and take over, or do it themselves.
fn fix_card(ui: &mut Ui, fix: &super::super::access::Fix, fixer: Option<Fixer>) -> Option<Action> {
    let mut action = None;
    Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 18))
        .stroke(Stroke::new(1.0, theme::alpha(theme::ACCENT(), 110)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(fix.title()).size(14.0).strong().color(theme::FG()));
            ui.add_space(2.0);
            let text = match fixer {
                None => fix.explanation(),
                Some(Fixer::Agent) => "The agent is making the fix on GitHub in a browser panel. If GitHub asks you to sign in, the agent hands the browser to you: sign in there, then press Done.".to_owned(),
                Some(Fixer::Person) => "Make the fix in the GitHub browser panel, then prove again.".to_owned(),
            };
            ui.label(RichText::new(text).size(13.0).color(theme::FG_SOFT()));
            ui.add_space(8.0);
            ui.horizontal(|ui| match fixer {
                None => {
                    let primary = egui::Button::new(
                        RichText::new("Let the agent do it")
                            .size(14.0)
                            .strong()
                            .color(theme::BG()),
                    )
                    .fill(theme::ACCENT());
                    if ui.add(primary.min_size(Vec2::new(170.0, 36.0)).corner_radius(10)).clicked() {
                        action = Some(Action::Fix(Fixer::Agent));
                    }
                    if ui.add(fix_button("Open GitHub myself")).clicked() {
                        action = Some(Action::Fix(Fixer::Person));
                    }
                }
                Some(fixer) => {
                    if ui.add(fix_button("Show browser")).clicked() {
                        action = Some(Action::ShowFix);
                    }
                    if fixer == Fixer::Person && ui.add(fix_button("Prove again")).clicked() {
                        action = Some(Action::ProveAgain);
                    }
                }
            });
        });
    action
}

fn fix_button(text: &str) -> egui::Button<'_> {
    action_button(text).min_size(Vec2::new(150.0, 36.0)).corner_radius(10)
}

fn callout(ui: &mut Ui, question: &str) {
    Frame::new()
        .fill(theme::alpha(theme::PALETTE_YELLOW(), 22))
        .stroke(Stroke::new(1.0, theme::alpha(theme::PALETTE_YELLOW(), 110)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("The agent asks").strong().color(theme::PALETTE_YELLOW()));
                ui.label(RichText::new(question).color(theme::FG()));
            });
        });
}

/// The steering field with Send, then Show agent and Stop.
fn working_actions(ui: &mut Ui, run: &mut Run, working: bool, inline: bool) -> Option<Action> {
    let mut action = None;
    if working && run.dotnet {
        ui.checkbox(
            &mut run.host_restore,
            RichText::new("Restore private NuGet packages on this computer for the proof").size(13.0),
        )
        .on_hover_text(
            "Runs dotnet restore here, with this computer's feed logins and the repository's build files, before the next proof; the container then reads the packages without any credential.",
        );
        ui.add_space(6.0);
    }
    if working && !run.crates.locked.is_empty() {
        let names = run.crates.locked.join(", ");
        let changed = ui
            .checkbox(
                &mut run.crates.allowed,
                RichText::new("Fetch private Rust git dependencies on this computer for the proof").size(13.0),
            )
            .on_hover_text(
                "Clones the GitHub repositories named below here with this computer's Git login before the next proof, and no other. The container reads each clone read-only, without any credential, and the proof's CI can read all of it. Clearing it stops clones a running proof has not begun.",
            )
            .changed();
        if changed && !run.crates.allowed {
            // A proof that is building now clones nothing more.
            run.crates.live.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        ui.label(RichText::new(names).size(12.0).color(theme::FG_SOFT()));
        ui.add_space(6.0);
    }
    if working && inline {
        let mut send = false;
        ui.horizontal(|ui| {
            let hint = if run.status.question.is_some() {
                "Answer the agent…"
            } else {
                "Tell the agent something, for example: skip the UI tests"
            };
            let field = ui.add(
                TextEdit::singleline(&mut run.steer)
                    .hint_text(RichText::new(hint).color(theme::FG_DIM()))
                    .font(egui::FontId::proportional(15.0))
                    .desired_width(ui.available_width() - BUTTON.x - 10.0)
                    .min_size(Vec2::new(0.0, 38.0))
                    .margin(egui::Margin::symmetric(12, 10)),
            );
            if field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                send = true;
                field.request_focus();
            }
            send |= ui
                .add_enabled(!run.steer.trim().is_empty(), dialog_button(action_button("Send")))
                .clicked();
        });
        let text = run.steer.trim().to_owned();
        if send && !text.is_empty() {
            run.steer.clear();
            action = Some(Action::Steer(text));
        }
        ui.add_space(8.0);
    } else if !working {
        ui.label(
            RichText::new("The setup agent stopped before it finished.")
                .size(14.0)
                .color(theme::PALETTE_RED()),
        );
        ui.add_space(8.0);
    }
    ui.horizontal(|ui| {
        if inline && ui.add(dialog_button(action_button("Show agent"))).clicked() {
            action = Some(Action::Show);
        }
        if run.preview_panel.is_some() && ui.add(dialog_button(action_button("Show live preview"))).clicked() {
            action = Some(Action::ShowPreview);
        }
        let stop = if working { "Stop" } else { "Back to the choices" };
        // Use cannot be cancelled once it started, so Stop waits until it reported.
        if ui
            .add_enabled(!run.ending(), dialog_button(danger_button(stop)))
            .clicked()
        {
            action = Some(Action::Stop);
        }
    });
    action
}

fn finished_actions(ui: &mut Ui, run: &Run, inline: bool) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(run.files.as_deref().unwrap_or_default())
                .monospace()
                .size(12.5)
                .color(theme::FG_SOFT()),
        );
        // Use this setup is in the footer beside the terminal; Stop ends a proved setup
        // the person does not want, in both layouts.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if inline {
                let primary = egui::Button::new(RichText::new("Use this setup").size(14.0).strong().color(theme::BG()))
                    .fill(theme::ACCENT());
                if ui.add_enabled(!run.ending(), dialog_button(primary)).clicked() {
                    action = Some(Action::Use);
                }
                if ui.add(dialog_button(action_button("Show agent"))).clicked() {
                    action = Some(Action::Show);
                }
            }
            if run.preview_panel.is_some() && ui.add(dialog_button(action_button("Show live preview"))).clicked() {
                action = Some(Action::ShowPreview);
            }
            // Use cannot be cancelled once it started, so Stop waits until it reported.
            if ui
                .add_enabled(!run.ending(), dialog_button(danger_button("Stop")))
                .clicked()
            {
                action = Some(Action::Stop);
            }
        });
    });
    action
}

fn dialog_button(button: egui::Button<'_>) -> egui::Button<'_> {
    button.min_size(BUTTON).corner_radius(10)
}

/// The agent's screen, sampled from its hidden panel, in a log well like a
/// deployment's output. The newest rows stay in view.
fn output_well(ui: &mut Ui, run: &Run, height: f32) {
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Agent output").size(14.0).strong().color(theme::FG()));
                ui.label(
                    RichText::new(format!("{} · live", run.agent))
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt("cloud-setup-output")
                .max_height(height)
                .min_scrolled_height(height)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if run.output.is_empty() {
                        ui.label(RichText::new("Starting…").monospace().size(12.5).color(theme::FG_DIM()));
                    }
                    for line in &run.output {
                        ui.add(
                            egui::Label::new(RichText::new(line).monospace().size(12.5).color(theme::FG_SOFT()))
                                .truncate(),
                        );
                    }
                });
        });
}

/// The agent's screen rows worth reading: no empty rows and no box-drawing rules.
pub(in crate::app::cloud_panel::production::repository_setup) fn readable_rows(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| {
            let visible = line.trim();
            !visible.is_empty()
                && !visible
                    .chars()
                    .all(|c| matches!(c, '─' | '━' | '│' | '╭' | '╮' | '╰' | '╯' | '-' | '=' | ' '))
        })
        .map(str::to_owned)
        .collect()
}

fn badge(ui: &mut Ui, color: Color32, glyph: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 10.0, theme::alpha(color, 50));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(13.0),
        color,
    );
}

fn chip(ui: &mut Ui, text: &str) {
    Frame::new()
        .fill(theme::alpha(theme::ACCENT(), 30))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).color(theme::ACCENT())));
}
