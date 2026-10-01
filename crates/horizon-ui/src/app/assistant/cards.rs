//! Activity cards: what the assistant did to other agents, approvals it needs,
//! and notes it posts. Horizon builds every card from host facts. The only text
//! an agent authors is a note's markdown, which is rendered without any action.

use std::time::{Duration, Instant};

use egui::{Margin, RichText, ScrollArea, Ui};
use egui_commonmark::CommonMarkViewer;
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::{
    HorizonApp,
    blocks::{self, Tone},
    icons::Icon,
};
use crate::theme;

const MAX_CARDS: usize = 12;
/// An approval nobody answered is dropped after this long.
const APPROVAL_LIFETIME: Duration = Duration::from_secs(600);
/// How long after a send an idle agent counts as having replied.
const REPLY_AFTER: Duration = Duration::from_secs(3);
const REPLY_ROWS: usize = 10;
const REPLY_LINES_SHOWN: usize = 6;
const SHOWN_CHARS: usize = 400;

#[derive(Clone)]
pub(super) enum CardKind {
    /// A message the assistant wants to type, waiting for the person.
    Approval {
        target: PanelId,
        title: String,
        text: String,
        submit: bool,
    },
    /// A message that was typed, with the agent's reply once it is idle again.
    Sent {
        target: PanelId,
        title: String,
        text: String,
        sent_at: Instant,
        reply: Option<String>,
    },
    /// A message that was not typed, and why.
    Declined { title: String, reason: String },
    /// A markdown note the assistant posted.
    Note { title: String, markdown: String },
}

pub(super) struct Card {
    id: u64,
    pub(super) kind: CardKind,
    created: Instant,
}

#[derive(Default)]
pub(super) struct Cards {
    items: Vec<Card>,
    next_id: u64,
}

impl Cards {
    pub(super) fn push(&mut self, kind: CardKind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Card {
            id,
            kind,
            created: Instant::now(),
        });
        while self.items.len() > MAX_CARDS {
            // Never drop a card that is waiting on the person.
            let Some(index) = self
                .items
                .iter()
                .position(|card| !matches!(card.kind, CardKind::Approval { .. }))
            else {
                break;
            };
            self.items.remove(index);
        }
        id
    }

    pub(super) fn dismiss(&mut self, id: u64) {
        self.items.retain(|card| card.id != id);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(super) fn first_kind(&self) -> Option<&CardKind> {
        self.items.first().map(|card| &card.kind)
    }

    #[cfg(test)]
    pub(super) fn first_id(&self) -> Option<u64> {
        self.items.first().map(|card| card.id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Card> {
        self.items.iter_mut().find(|card| card.id == id)
    }

    fn expire(&mut self, now: Instant) {
        self.items.retain(|card| {
            !matches!(card.kind, CardKind::Approval { .. }) || now.duration_since(card.created) < APPROVAL_LIFETIME
        });
    }
}

/// The last few non-empty rows of an agent's terminal, bounded for display.
fn reply_tail(text: &str) -> String {
    let rows: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|row| !row.trim().is_empty())
        .collect();
    let start = rows.len().saturating_sub(REPLY_LINES_SHOWN);
    shorten(&rows[start..].join("\n"))
}

fn shorten(text: &str) -> String {
    if text.chars().count() <= SHOWN_CHARS {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(SHOWN_CHARS).collect();
    shortened.push_str("...");
    shortened
}

enum CardAction {
    Approve(u64),
    Decline(u64),
    Dismiss(u64),
    Reveal(PanelId),
}

impl HorizonApp {
    pub(in crate::app) fn assistant_asks_before_send(&self) -> bool {
        self.assistant.settings.ask_before_send
    }

    /// Queues a message for the person to approve and makes sure they can see it.
    pub(in crate::app) fn assistant_request_approval(
        &mut self,
        target: PanelId,
        title: String,
        text: String,
        submit: bool,
    ) {
        self.assistant.cards.push(CardKind::Approval {
            target,
            title,
            text,
            submit,
        });
        self.assistant.open = true;
    }

    pub(in crate::app) fn assistant_record_sent(&mut self, target: PanelId, title: String, text: String) {
        self.assistant.cards.push(CardKind::Sent {
            target,
            title,
            text,
            sent_at: Instant::now(),
            reply: None,
        });
    }

    pub(in crate::app) fn assistant_post_note(&mut self, title: String, markdown: String) {
        self.assistant.cards.push(CardKind::Note { title, markdown });
        self.assistant.open = true;
    }

    /// Expires old approvals and captures the reply of agents that went idle again.
    pub(super) fn update_assistant_cards(&mut self) {
        self.assistant.cards.expire(Instant::now());
        let waiting: Vec<(u64, PanelId)> = self
            .assistant
            .cards
            .items
            .iter()
            .filter_map(|card| match &card.kind {
                CardKind::Sent {
                    target,
                    sent_at,
                    reply: None,
                    ..
                } if sent_at.elapsed() >= REPLY_AFTER => Some((card.id, *target)),
                _ => None,
            })
            .collect();
        for (id, target) in waiting {
            let reply = match self.board.agent_state(target) {
                Some(AgentState::Idle) => self
                    .board
                    .agent_output(target, REPLY_ROWS)
                    .map(|(text, _)| reply_tail(&text)),
                None | Some(AgentState::Exited) => Some("The agent closed before replying.".to_string()),
                Some(_) => None,
            };
            if let Some(reply) = reply
                && let Some(Card {
                    kind: CardKind::Sent { reply: slot, .. },
                    ..
                }) = self.assistant.cards.get_mut(id)
            {
                *slot = Some(reply);
            }
        }
    }

    /// Settles an approval: type the message if the person agreed and the agent can still take it.
    pub(super) fn resolve_approval(&mut self, id: u64, approve: bool) {
        let Some(CardKind::Approval {
            target,
            title,
            text,
            submit,
        }) = self.assistant.cards.get_mut(id).map(|card| card.kind.clone())
        else {
            return;
        };
        let outcome = if !approve {
            Err("You declined this message.".to_string())
        } else if let Some(assistant) = self.board.assistant_panel() {
            self.board
                .check_agent_can_receive(assistant, target)
                .map_err(|refusal| refusal.message().to_string())
                .and_then(|()| {
                    self.send_to_agent(target, &text, submit, Instant::now())
                        .then_some(())
                        .ok_or_else(|| "The agent is no longer available.".to_string())
                })
        } else {
            Err("The assistant is not running.".to_string())
        };
        let kind = match outcome {
            Ok(()) => CardKind::Sent {
                target,
                title,
                text,
                sent_at: Instant::now(),
                reply: None,
            },
            Err(reason) => CardKind::Declined { title, reason },
        };
        if let Some(card) = self.assistant.cards.get_mut(id) {
            card.kind = kind;
        }
    }

    pub(super) fn render_cards_tray(&mut self, ui: &mut Ui) {
        self.update_assistant_cards();
        if self.assistant.cards.is_empty() {
            return;
        }
        let max_height = (ui.available_height() * 0.45).max(150.0);
        let mut actions = Vec::new();
        egui::Frame::new()
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("ACTIVITY")
                        .size(10.5)
                        .extra_letter_spacing(0.9)
                        .color(theme::FG_DIM()),
                );
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .max_height(max_height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for card in &self.assistant.cards.items {
                            let live = match &card.kind {
                                CardKind::Sent { target, .. } => self.board.agent_state(*target),
                                _ => None,
                            };
                            draw_card(ui, card, live, &mut self.assistant.md_cache, &mut actions);
                            ui.add_space(8.0);
                        }
                    });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1.0, theme::BORDER_SUBTLE()),
        );
        for action in actions {
            match action {
                CardAction::Approve(id) => self.resolve_approval(id, true),
                CardAction::Decline(id) => self.resolve_approval(id, false),
                CardAction::Dismiss(id) => self.assistant.cards.dismiss(id),
                CardAction::Reveal(target) => self.reveal_selected_panel(ui.ctx(), target),
            }
        }
    }
}

fn draw_card(
    ui: &mut Ui,
    card: &Card,
    live: Option<AgentState>,
    markdown_cache: &mut egui_commonmark::CommonMarkCache,
    actions: &mut Vec<CardAction>,
) {
    match &card.kind {
        CardKind::Approval { title, text, .. } => {
            blocks::card(ui, Tone::Attention, |ui| {
                let pill = Some(("Needs your OK", theme::PALETTE_YELLOW()));
                blocks::header(
                    ui,
                    Icon::Send,
                    Tone::Attention,
                    &format!("Send to {title}?"),
                    "The assistant wants to type this",
                    pill,
                );
                ui.add_space(8.0);
                quoted(ui, text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if blocks::primary(ui, "Send").clicked() {
                        actions.push(CardAction::Approve(card.id));
                    }
                    if blocks::ghost(ui, "Don't send").clicked() {
                        actions.push(CardAction::Decline(card.id));
                    }
                });
            });
        }
        CardKind::Sent {
            target,
            title,
            text,
            reply,
            ..
        } => {
            let (tone, icon, pill) = match (reply, live) {
                (Some(_), _) => (Tone::Good, Icon::Check, ("Replied", theme::PALETTE_GREEN())),
                (None, Some(AgentState::Working)) => (Tone::Neutral, Icon::Bot, ("Working", theme::ACCENT())),
                (None, Some(AgentState::NeedsInput)) => {
                    (Tone::Neutral, Icon::Bot, ("Needs input", theme::PALETTE_YELLOW()))
                }
                (None, None | Some(AgentState::Exited)) => (Tone::Neutral, Icon::Bot, ("Closed", theme::PALETTE_RED())),
                (None, Some(_)) => (Tone::Neutral, Icon::Bot, ("Sent", theme::FG_DIM())),
            };
            blocks::card(ui, tone, |ui| {
                blocks::header(ui, icon, tone, title, "Message sent", Some(pill));
                ui.add_space(8.0);
                ui.label(blocks::mono(format!("> {}", shorten(text)), theme::FG_SOFT()));
                if let Some(reply) = reply {
                    ui.add_space(4.0);
                    ui.label(blocks::mono(reply.as_str(), theme::PALETTE_GREEN()));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if blocks::ghost(ui, "Show agent").clicked() {
                        actions.push(CardAction::Reveal(*target));
                    }
                    if reply.is_some() && blocks::ghost(ui, "Dismiss").clicked() {
                        actions.push(CardAction::Dismiss(card.id));
                    }
                });
            });
        }
        CardKind::Declined { title, reason } => {
            blocks::card(ui, Tone::Neutral, |ui| {
                blocks::header(
                    ui,
                    Icon::Bot,
                    Tone::Neutral,
                    &format!("Not sent to {title}"),
                    reason,
                    None,
                );
                ui.add_space(8.0);
                if blocks::ghost(ui, "Dismiss").clicked() {
                    actions.push(CardAction::Dismiss(card.id));
                }
            });
        }
        CardKind::Note { title, markdown } => {
            blocks::card(ui, Tone::Neutral, |ui| {
                blocks::header(ui, Icon::Note, Tone::Neutral, title, "From the assistant", None);
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.visuals_mut().override_text_color = Some(theme::FG_SOFT());
                    CommonMarkViewer::new().show(ui, markdown_cache, markdown);
                });
                ui.add_space(8.0);
                if blocks::ghost(ui, "Dismiss").clicked() {
                    actions.push(CardAction::Dismiss(card.id));
                }
            });
        }
    }
}

/// The message in a recessed block, so it reads as quoted text.
fn quoted(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .fill(theme::PANEL_BG())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(shorten(text)).size(12.5).color(theme::FG_SOFT()));
        });
}

#[cfg(test)]
mod tests;
