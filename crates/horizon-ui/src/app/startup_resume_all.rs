use egui::{Align2, Color32, Margin, RichText, Stroke, Vec2};

use crate::theme;

use super::util::{chrome_button, primary_button};
use super::{HorizonApp, ResolvedSession};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeAllAction {
    None,
    ResumeAll,
    StartFresh,
}

impl HorizonApp {
    /// Queue the resume-all prompt for a restored session that carries agent
    /// panels; returns true when activation was deferred to the prompt.
    pub(super) fn maybe_queue_resume_all_prompt(&mut self, session: &ResolvedSession) -> bool {
        if session.runtime_state.resume_all_candidate_count() == 0 {
            return false;
        }
        self.pending_resume_all = Some(session.clone());
        true
    }

    /// Gate the frame while the resume-all prompt is on screen; returns true
    /// once the user has chosen (and the session is activating) or no prompt
    /// is pending.
    pub(super) fn prepare_resume_all_prompt(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(session) = self.pending_resume_all.take() else {
            return true;
        };

        let action = render_resume_all_prompt(ui, session.runtime_state.resume_all_candidate_count());
        if matches!(action, ResumeAllAction::None) {
            self.pending_resume_all = Some(session);
            return false;
        }

        let mut session = session;
        if action == ResumeAllAction::StartFresh {
            session.runtime_state.start_agent_panels_fresh();
        } else {
            session.runtime_state.apply_resume_all_agent_panels();
        }
        let ctx = ui.ctx().clone();
        self.activate_persistent_session(&session);
        self.restore_window_viewport(&ctx);
        true
    }
}

fn render_resume_all_prompt(ui: &mut egui::Ui, panel_count: usize) -> ResumeAllAction {
    let mut action = ResumeAllAction::None;
    render_prompt_card(ui, panel_count, &mut action);
    if matches!(action, ResumeAllAction::None) {
        action = ui.input(|input| {
            if input.key_pressed(egui::Key::Enter) {
                ResumeAllAction::ResumeAll
            } else if input.key_pressed(egui::Key::Escape) {
                ResumeAllAction::StartFresh
            } else {
                ResumeAllAction::None
            }
        });
    }
    action
}

fn render_prompt_card(ui: &mut egui::Ui, panel_count: usize, action: &mut ResumeAllAction) {
    let prompt_text = match panel_count {
        1 => "This session has 1 agent panel that can reattach to its previous session.".to_string(),
        _ => format!("This session has {panel_count} agent panels that can reattach to their previous sessions."),
    };

    egui::Window::new("resume_all_prompt")
        .title_bar(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .fixed_size(Vec2::new(640.0, 0.0))
        .order(egui::Order::Debug)
        .frame(
            egui::Frame::NONE
                .fill(theme::PANEL_BG())
                .stroke(Stroke::new(1.5_f32, theme::alpha(theme::ACCENT(), 80)))
                .corner_radius(egui::CornerRadius::same(20))
                .shadow(egui::Shadow {
                    offset: [0, 12],
                    blur: 32,
                    spread: 2,
                    color: Color32::from_black_alpha(132),
                }),
        )
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE.inner_margin(Margin::same(22)).show(ui, |ui| {
                ui.label(
                    RichText::new("Resume agent sessions?")
                        .size(18.0)
                        .strong()
                        .color(theme::FG()),
                );
                ui.add_space(10.0);
                ui.label(RichText::new(prompt_text).size(12.5).color(theme::FG_SOFT()));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Resume all of them, or start every agent panel fresh?")
                        .size(12.5)
                        .color(theme::FG_SOFT()),
                );
                ui.add_space(20.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(chrome_button("Start fresh")).clicked() {
                        *action = ResumeAllAction::StartFresh;
                    }
                    if ui.add(primary_button("Resume all")).clicked() {
                        *action = ResumeAllAction::ResumeAll;
                    }
                });
                ui.add_space(12.0);
                ui.label(
                    RichText::new("Enter — resume all · Esc — start fresh")
                        .size(11.0)
                        .color(theme::FG_DIM()),
                );
            });
        });
}
