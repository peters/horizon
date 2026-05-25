use std::cmp::Reverse;

use egui::{Align, Color32, Context, Layout, Margin, RichText, ScrollArea, Stroke, TextEdit, Ui, Vec2};
use horizon_core::{AgentPairRole, FindingCard, FindingStatus};

use super::state::{LinkablePanel, dispatch_enabled, role_heading, shorten_middle};
use super::{
    AGENT_PAIR_REVIEW_QUEUE_DEFAULT_WIDTH, AGENT_PAIR_REVIEW_QUEUE_MAX_WIDTH, AGENT_PAIR_REVIEW_QUEUE_MIN_WIDTH,
    AGENT_PAIR_REVIEW_QUEUE_PANEL_ID, HorizonApp, card_status_order,
};
use crate::app::util::{chrome_button, danger_button, primary_button};
use crate::theme;

enum QueueAction {
    Close,
    Link(AgentPairRole, Option<String>),
    CreateCandidate,
    Accept(String),
    Reject(String),
    Dispatch(String),
    Verify(String),
    Focus(String),
}

impl HorizonApp {
    pub(in crate::app) fn render_agent_pair_review_queue(&mut self, ctx: &Context) {
        if !self.agent_pair_review_queue_open {
            return;
        }

        let linkable_panels = self.linkable_agent_panels();
        let mut actions = Vec::new();

        egui::SidePanel::right(AGENT_PAIR_REVIEW_QUEUE_PANEL_ID)
            .resizable(true)
            .default_width(AGENT_PAIR_REVIEW_QUEUE_DEFAULT_WIDTH)
            .width_range(AGENT_PAIR_REVIEW_QUEUE_MIN_WIDTH..=AGENT_PAIR_REVIEW_QUEUE_MAX_WIDTH)
            .frame(
                egui::Frame::default()
                    .fill(theme::PANEL_BG())
                    .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 210)))
                    .inner_margin(Margin::same(14)),
            )
            .show(ctx, |ui| {
                ui.set_min_width(320.0);
                render_title(ui, &mut actions);
                ui.add_space(10.0);
                self.render_agent_links(ui, &linkable_panels, &mut actions);
                ui.add_space(12.0);
                self.render_candidate_form(ui, &mut actions);
                ui.add_space(12.0);
                self.render_cards(ui, &mut actions);
            });

        for action in actions {
            self.apply_queue_action(ctx, action);
        }
    }

    fn render_agent_links(&self, ui: &mut Ui, panels: &[LinkablePanel], actions: &mut Vec<QueueAction>) {
        ui.horizontal_wrapped(|ui| {
            render_agent_link_chip(self, ui, AgentPairRole::Researcher, panels, actions);
            render_agent_link_chip(self, ui, AgentPairRole::Performer, panels, actions);
        });
    }

    fn render_candidate_form(&mut self, ui: &mut Ui, actions: &mut Vec<QueueAction>) {
        egui::CollapsingHeader::new(RichText::new("New Candidate").color(theme::FG()).size(13.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                labeled_singleline(ui, "Title", &mut self.agent_pair_ui.candidate.title);
                labeled_multiline(ui, "Summary", &mut self.agent_pair_ui.candidate.summary, 2);
                labeled_multiline(ui, "Evidence", &mut self.agent_pair_ui.candidate.evidence, 3);
                labeled_multiline(
                    ui,
                    "Suspected files",
                    &mut self.agent_pair_ui.candidate.suspected_files,
                    3,
                );
                labeled_multiline(
                    ui,
                    "Suggested tests",
                    &mut self.agent_pair_ui.candidate.suggested_tests,
                    3,
                );

                let ready = self.agent_pair_ui.candidate.is_ready();
                if ui
                    .add_enabled(ready, primary_button("Add Candidate").min_size(Vec2::new(124.0, 30.0)))
                    .clicked()
                {
                    actions.push(QueueAction::CreateCandidate);
                }
            });
    }

    fn render_cards(&mut self, ui: &mut Ui, actions: &mut Vec<QueueAction>) {
        if let Some(error) = &self.agent_pair_ui.error {
            ui.label(RichText::new(error).color(theme::PALETTE_RED()).size(11.0));
            ui.add_space(8.0);
        }

        let mut cards = self.agent_pair_queue.cards.clone();
        cards.sort_by_key(|card| (card_status_order(card.status), Reverse(card.updated_at_millis)));

        ui.horizontal(|ui| {
            ui.label(RichText::new("Findings").color(theme::FG()).size(13.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(cards.len().to_string())
                        .monospace()
                        .color(theme::FG_DIM())
                        .size(11.0),
                );
            });
        });
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt("agent_pair_review_queue_cards")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if cards.is_empty() {
                    ui.label(RichText::new("No findings queued.").color(theme::FG_DIM()).size(11.0));
                    return;
                }

                for card in cards {
                    self.render_card(ui, &card, actions);
                    ui.add_space(10.0);
                }
            });
    }

    fn render_card(&mut self, ui: &mut Ui, card: &FindingCard, actions: &mut Vec<QueueAction>) {
        egui::Frame::default()
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 190)))
            .corner_radius(8)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(status_badge(card.status));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(shorten_middle(&card.id, 12))
                                .monospace()
                                .color(theme::FG_DIM())
                                .size(10.0),
                        );
                    });
                });
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(&card.title).color(theme::FG()).size(13.0).strong()).wrap());
                ui.add_space(4.0);
                wrapping_label(ui, &card.summary, theme::FG_SOFT());
                ui.add_space(6.0);
                wrapping_label(ui, &card.evidence, theme::FG_DIM());
                render_list(ui, "Files", &card.suspected_files);
                render_list(ui, "Tests", &card.suggested_tests);

                let performer_title = self.performer_title_for_card(&card.id);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(card.assignment_label(performer_title.as_deref()))
                        .color(theme::FG_SOFT())
                        .size(11.0),
                );
                ui.add_space(8.0);
                self.render_card_actions(ui, card, actions);
            });
    }

    fn render_card_actions(&mut self, ui: &mut Ui, card: &FindingCard, actions: &mut Vec<QueueAction>) {
        match card.status {
            FindingStatus::Candidate => {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add(primary_button("Accept").min_size(Vec2::new(74.0, 28.0)))
                        .clicked()
                    {
                        actions.push(QueueAction::Accept(card.id.clone()));
                    }
                    if ui
                        .add(danger_button("Reject").min_size(Vec2::new(74.0, 28.0)))
                        .clicked()
                    {
                        actions.push(QueueAction::Reject(card.id.clone()));
                    }
                });
            }
            FindingStatus::Accepted => {
                let enabled = dispatch_enabled(&self.agent_pair_queue, card);
                if ui
                    .add_enabled(enabled, primary_button("Dispatch").min_size(Vec2::new(96.0, 28.0)))
                    .on_disabled_hover_text("Link a performer panel before dispatch.")
                    .clicked()
                {
                    actions.push(QueueAction::Dispatch(card.id.clone()));
                }
            }
            FindingStatus::Implementing => self.render_evidence_form(ui, card, actions),
            FindingStatus::Verified | FindingStatus::Rejected => {}
        }
    }

    fn render_evidence_form(&mut self, ui: &mut Ui, card: &FindingCard, actions: &mut Vec<QueueAction>) {
        let draft = self.agent_pair_ui.evidence_draft_mut(card);
        ui.separator();
        ui.add_space(4.0);
        labeled_multiline(ui, "Verification", &mut draft.verification_summary, 2);
        labeled_multiline(ui, "Commands", &mut draft.validation_commands, 3);
        labeled_multiline(ui, "Result", &mut draft.validation_result, 2);
        labeled_multiline(ui, "Regression scope", &mut draft.regression_scope, 2);
        let complete = draft.packet().is_complete();
        if ui
            .add_enabled(
                complete,
                primary_button("Mark Verified").min_size(Vec2::new(118.0, 28.0)),
            )
            .clicked()
        {
            actions.push(QueueAction::Verify(card.id.clone()));
        }
    }

    fn apply_queue_action(&mut self, ctx: &Context, action: QueueAction) {
        match action {
            QueueAction::Close => self.agent_pair_review_queue_open = false,
            QueueAction::Link(role, panel_local_id) => self.link_agent_panel(role, panel_local_id),
            QueueAction::CreateCandidate => self.create_agent_pair_candidate(),
            QueueAction::Accept(finding_id) => self.accept_agent_pair_card(&finding_id),
            QueueAction::Reject(finding_id) => self.reject_agent_pair_card(&finding_id),
            QueueAction::Dispatch(finding_id) => self.dispatch_agent_pair_card(&finding_id),
            QueueAction::Verify(finding_id) => self.verify_agent_pair_card(&finding_id),
            QueueAction::Focus(panel_local_id) => self.focus_linked_agent_panel(ctx, &panel_local_id),
        }
    }
}

fn render_title(ui: &mut Ui, actions: &mut Vec<QueueAction>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Review Queue").color(theme::FG()).size(16.0).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(chrome_button("Close").min_size(Vec2::new(64.0, 28.0))).clicked() {
                actions.push(QueueAction::Close);
            }
        });
    });
}

fn render_agent_link_chip(
    app: &HorizonApp,
    ui: &mut Ui,
    role: AgentPairRole,
    panels: &[LinkablePanel],
    actions: &mut Vec<QueueAction>,
) {
    let linked_id = app
        .agent_pair_queue
        .link_for(role)
        .map(|link| link.panel_local_id.as_str());
    let current = linked_id.and_then(|local_id| panels.iter().find(|panel| panel.local_id == local_id));

    egui::Frame::default()
        .fill(theme::alpha(theme::BG_ELEVATED(), 210))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 190)))
        .corner_radius(8)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(190.0);
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.label(RichText::new(role_heading(role)).color(theme::FG()).size(11.5).strong());
                ui.add_space(2.0);
                ui.label(link_label(current, linked_id).color(theme::FG_SOFT()).size(10.5));
                ui.label(RichText::new(link_detail(current)).color(theme::FG_DIM()).size(10.0));
                ui.add_space(6.0);
                egui::ComboBox::from_id_salt(("agent_pair_link", role.label()))
                    .selected_text(current.map_or("Disconnected".to_string(), |panel| shorten_middle(&panel.title, 22)))
                    .width(168.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(linked_id.is_none(), "Disconnected").clicked() {
                            actions.push(QueueAction::Link(role, None));
                        }
                        for panel in panels {
                            let selected = Some(panel.local_id.as_str()) == linked_id;
                            let label = format!("{} · {}", shorten_middle(&panel.title, 24), panel.kind.display_name());
                            if ui.selectable_label(selected, label).clicked() {
                                actions.push(QueueAction::Link(role, Some(panel.local_id.clone())));
                            }
                        }
                    });
                let focus_enabled = current.is_some();
                if ui
                    .add_enabled(focus_enabled, chrome_button("Focus").min_size(Vec2::new(68.0, 26.0)))
                    .clicked()
                    && let Some(panel) = current
                {
                    actions.push(QueueAction::Focus(panel.local_id.clone()));
                }
            });
        });
}

fn link_label(current: Option<&LinkablePanel>, linked_id: Option<&str>) -> RichText {
    match (current, linked_id) {
        (Some(panel), _) => RichText::new(format!(
            "{} · {}",
            panel.kind.display_name(),
            shorten_middle(&panel.title, 24)
        )),
        (None, Some(_) | None) => RichText::new("Disconnected"),
    }
}

fn link_detail(current: Option<&LinkablePanel>) -> String {
    current.map_or_else(
        || "No linked panel".to_string(),
        |panel| {
            let terminal = if panel.terminal_backed {
                "terminal"
            } else {
                "not terminal"
            };
            format!(
                "{} · panel {} · {terminal}",
                shorten_middle(&panel.workspace_name, 24),
                panel.panel_id.0
            )
        },
    )
}

fn labeled_singleline(ui: &mut Ui, label: &str, value: &mut String) {
    ui.label(RichText::new(label).color(theme::FG_DIM()).size(10.5));
    ui.add(
        TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .font(egui::FontId::proportional(12.0)),
    );
}

fn labeled_multiline(ui: &mut Ui, label: &str, value: &mut String, rows: usize) {
    ui.label(RichText::new(label).color(theme::FG_DIM()).size(10.5));
    ui.add(
        TextEdit::multiline(value)
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .font(egui::FontId::proportional(12.0)),
    );
}

fn wrapping_label(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(color).size(11.0)).wrap());
}

fn render_list(ui: &mut Ui, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.label(RichText::new(label).color(theme::FG_DIM()).size(10.5));
    for value in values {
        wrapping_label(ui, &shorten_middle(value, 72), theme::FG_SOFT());
    }
}

fn status_badge(status: FindingStatus) -> RichText {
    let color = match status {
        FindingStatus::Candidate => theme::ACCENT(),
        FindingStatus::Accepted | FindingStatus::Verified => theme::PALETTE_GREEN(),
        FindingStatus::Rejected => theme::PALETTE_RED(),
        FindingStatus::Implementing => theme::PALETTE_YELLOW(),
    };
    RichText::new(status.label())
        .monospace()
        .color(color)
        .size(10.5)
        .strong()
}
