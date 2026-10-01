//! The conversation as a card for each ask (design B), and the pieces the deck (design C) shares.
//!
//! A turn is one ask and what followed: the steps the agent took and what it said at the end. The
//! card puts the outcome first, as a title and a short body, and keeps the steps one click away. The
//! raw terminal stays one click away too.

use egui::{Align, CornerRadius, Frame, Id, Layout, Margin, RichText, Stroke, Ui};
use egui_commonmark::CommonMarkViewer;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::blocks;
use super::feed::Turn;
use super::{HorizonApp, desk_bar::paint::elide};
use crate::theme;

/// How the feed shows the conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum FeedStyle {
    /// Bubbles, with the steps as small pills between them.
    #[default]
    Chat,
    /// A card for each ask: the outcome first, the steps and the terminal one click away.
    Cards,
}

impl FeedStyle {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Cards => "Cards",
        }
    }
}

/// The first sentence of a reply, as a title, and the rest as its body.
pub(super) fn split_reply(reply: &str) -> (String, String) {
    let reply = reply.trim();
    let cut = reply
        .char_indices()
        .find(|(index, c)| {
            matches!(c, '.' | '!' | '?' | '\n')
                && reply[index + c.len_utf8()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map_or(reply.len(), |(index, c)| index + c.len_utf8());
    let (title, rest) = reply.split_at(cut);
    (elide(title.trim(), 96), rest.trim().to_string())
}

/// What kind of step a pill describes, for the counts on a card: its singular and plural names.
fn step_kind(step: &str) -> (&'static str, &'static str) {
    match step.split_whitespace().next().unwrap_or_default() {
        "Edited" => ("edit", "edits"),
        "Ran" | "Run" => ("command", "commands"),
        "Sent" => ("task sent", "tasks sent"),
        "Read" => ("read", "reads"),
        "Looked" | "Searched" => ("check", "checks"),
        "Planned" => ("plan", "plans"),
        "Posted" => ("recap", "recaps"),
        _ => ("other", "other"),
    }
}

/// `3 edits, 2 commands` for the steps of a turn.
pub(super) fn step_counts(steps: &[String]) -> String {
    let mut counts: Vec<((&'static str, &'static str), usize)> = Vec::new();
    for step in steps {
        let kind = step_kind(step);
        match counts.iter_mut().find(|(name, _)| *name == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind, 1)),
        }
    }
    // Most common first; ties keep the order they appeared in.
    counts.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    counts
        .iter()
        .take(3)
        .map(|((one, many), count)| format!("{count} {}", if *count == 1 { one } else { many }))
        .collect::<Vec<_>>()
        .join(", ")
}

impl HorizonApp {
    /// Whether the assistant or one of its agents is working right now.
    pub(super) fn assistant_busy(&self) -> bool {
        let assistant_working = self
            .board
            .assistant_panel()
            .is_some_and(|id| self.board.agent_state(id) == Some(AgentState::Working));
        assistant_working
            || self
                .feed_agents()
                .iter()
                .any(|agent| agent.state == AgentState::Working)
    }

    /// One card for each ask.
    pub(super) fn draw_turn_cards(&mut self, ui: &mut Ui) {
        let turns = self.assistant.feed.turns();
        let busy = self.assistant_busy();
        let last = turns.len().saturating_sub(1);
        let mut terminal = false;
        for (index, turn) in turns.iter().enumerate() {
            let working = busy && index == last;
            if self.turn_card(ui, index, turn, working) {
                terminal = true;
            }
            ui.add_space(10.0);
        }
        if terminal {
            self.assistant.summon.raw = true;
        }
    }

    /// Returns true when the card's Terminal button was pressed.
    fn turn_card(&mut self, ui: &mut Ui, index: usize, turn: &Turn, working: bool) -> bool {
        let mut terminal = false;
        let steps_id = Id::new(("turn_steps", index));
        let mut open = ui
            .memory(|memory| memory.data.get_temp::<bool>(steps_id))
            .unwrap_or(false);
        let accent = if working {
            theme::PALETTE_YELLOW()
        } else {
            theme::PALETTE_GREEN()
        };
        Frame::new()
            .fill(theme::BG_ELEVATED())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(asked) = &turn.asked {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if turn.voice { "YOU SAID" } else { "YOU ASKED" })
                                .size(10.0)
                                .extra_letter_spacing(0.9)
                                .color(theme::FG_DIM()),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(elide(asked, 150)).size(12.0).color(theme::FG_SOFT()))
                                .truncate(),
                        );
                    });
                    ui.add_space(8.0);
                }
                if let Some(reply) = turn.replies.last() {
                    let (title, body) = split_reply(reply);
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 22.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 5.0, accent);
                        ui.add(egui::Label::new(RichText::new(title).size(15.5).strong().color(theme::FG())).wrap());
                    });
                    if !body.is_empty() {
                        ui.add_space(4.0);
                        ui.scope(|ui| {
                            ui.visuals_mut().override_text_color = Some(theme::FG_SOFT());
                            CommonMarkViewer::new().show(ui, &mut self.assistant.md_cache, &body);
                        });
                    }
                } else if working {
                    ui.horizontal(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(14.0, 22.0), egui::Sense::hover());
                        let pulse = 0.6 + 0.4 * (super::super::num::seconds(ui) * 4.0).sin().abs();
                        ui.painter().circle_filled(dot.center(), 3.0 + 2.5 * pulse, accent);
                        ui.label(RichText::new("Working").size(15.0).strong().color(theme::FG()));
                    });
                    ui.ctx().request_repaint();
                }
                if working && let Some(step) = turn.steps.last() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(elide(step, 90)).size(12.0).color(theme::FG_DIM()));
                }
                if !turn.steps.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} steps", turn.steps.len()))
                                .size(11.5)
                                .color(theme::FG_DIM()),
                        );
                        let counts = step_counts(&turn.steps);
                        if !counts.is_empty() {
                            ui.label(RichText::new(counts).size(11.5).color(theme::FG_DIM()));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if blocks::ghost(ui, "Terminal").clicked() {
                                terminal = true;
                            }
                            if blocks::ghost(ui, if open { "Hide steps" } else { "Steps" }).clicked() {
                                open = !open;
                            }
                        });
                    });
                }
                if open {
                    ui.add_space(6.0);
                    for step in &turn.steps {
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 2.5, theme::ACCENT());
                            ui.label(RichText::new(elide(step, 110)).size(12.0).color(theme::FG_SOFT()));
                        });
                    }
                }
            });
        ui.memory_mut(|memory| memory.data.insert_temp(steps_id, open));
        terminal
    }
}

#[cfg(test)]
mod tests {
    use super::{split_reply, step_counts};

    #[test]
    fn a_reply_splits_into_a_title_and_a_body() {
        let (title, body) = split_reply("Fixed the test. It was a race in cleanup.\n- one\n- two");
        assert_eq!(title, "Fixed the test.");
        assert!(body.starts_with("It was a race"));
        let (title, body) = split_reply("Done");
        assert_eq!((title.as_str(), body.as_str()), ("Done", ""));
        // A version number is not the end of a sentence.
        assert_eq!(
            split_reply("Updated to 1.2.3 today. Next.").0,
            "Updated to 1.2.3 today."
        );
    }

    #[test]
    fn steps_are_counted_by_kind_most_common_first() {
        let steps: Vec<String> = ["Edited a.rs", "Edited b.rs", "Ran cargo test", "Sent a task to x"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(step_counts(&steps), "2 edits, 1 command, 1 task sent");
    }
}
