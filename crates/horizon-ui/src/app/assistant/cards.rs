//! Activity cards: what the assistant did to other agents, approvals it needs,
//! and notes it posts. Horizon builds every card from host facts. The only text
//! an agent authors is a note's markdown, which is rendered without any action.

use std::time::{Duration, Instant};

use egui::{Margin, RichText, ScrollArea, Ui};
use egui_commonmark::CommonMarkViewer;
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::{AgentState, Approval, ApprovalStatus};

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
const REPORT_CHARS: usize = 120;

#[derive(Clone)]
pub(super) enum CardKind {
    /// A message the assistant wants to type, waiting for the person.
    Approval {
        /// The agent's stable id, so a board that was replaced meanwhile cannot redirect the message.
        local_id: String,
        title: String,
        /// Agent kind and directory, so the target is not identified by a title it controls.
        detail: String,
        /// Exactly the text that will be typed.
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

    pub(super) fn clear(&mut self) {
        self.items.clear();
    }

    pub(super) fn pending_approvals(&self) -> usize {
        self.items
            .iter()
            .filter(|card| matches!(card.kind, CardKind::Approval { .. }))
            .count()
    }

    /// Each message that needed approval, with what became of it.
    pub(super) fn report(&self) -> Vec<Approval> {
        self.items
            .iter()
            .filter_map(|card| match &card.kind {
                CardKind::Approval { title, text, .. } => Some((title, text, ApprovalStatus::Pending, None)),
                CardKind::Sent { title, text, .. } => Some((title, text, ApprovalStatus::Sent, None)),
                CardKind::Declined { title, reason } => {
                    Some((title, reason, ApprovalStatus::Declined, Some(reason.clone())))
                }
                CardKind::Note { .. } => None,
            })
            .map(|(title, text, status, detail)| Approval {
                target: title.clone(),
                message: shorten_to(text, REPORT_CHARS),
                status,
                detail,
            })
            .collect()
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
    shorten_to(text, SHOWN_CHARS)
}

fn shorten_to(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(limit).collect();
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
    /// Returns false when the target is gone.
    pub(in crate::app) fn assistant_request_approval(&mut self, target: PanelId, text: String, submit: bool) -> bool {
        let Some(panel) = self.board.panel(target) else {
            return false;
        };
        let kind = horizon_core::agent_definition(panel.kind).map_or("agent", |agent| agent.display_name);
        let detail = match &panel.launch_cwd {
            Some(dir) => format!("{kind} - {}", dir.display()),
            None => kind.to_string(),
        };
        let card = CardKind::Approval {
            local_id: panel.local_id.clone(),
            title: panel.display_title().into_owned(),
            detail,
            text,
            submit,
        };
        self.assistant.cards.push(card);
        self.assistant.open = true;
        true
    }

    /// Notes something the assistant did, for the feed.
    pub(in crate::app) fn assistant_did(&mut self, text: String) {
        self.assistant.feed.did(text);
    }

    pub(in crate::app) fn assistant_record_sent(&mut self, target: PanelId, text: String) {
        if let Some(demo) = self.assistant.demo.as_ref() {
            demo.log("event sent");
        }
        let title = self
            .board
            .panel(target)
            .map_or_else(String::new, |panel| panel.display_title().into_owned());
        self.assistant.feed.did(format!("Sent a task to {title}"));
        self.assistant.cards.push(CardKind::Sent {
            target,
            title,
            text,
            sent_at: Instant::now(),
            reply: None,
        });
    }

    pub(in crate::app) fn assistant_pending_approvals(&self) -> usize {
        self.assistant.cards.pending_approvals()
    }

    /// What became of the messages that needed approval, for the assistant to read.
    pub(in crate::app) fn assistant_approvals(&self) -> Vec<Approval> {
        self.assistant.cards.report()
    }

    pub(in crate::app) fn assistant_post_note(&mut self, title: String, markdown: String) {
        if let Some(demo) = self.assistant.demo.as_ref() {
            demo.log(&format!("event note {title}"));
        }
        self.assistant.feed.did(format!("Posted a recap: {title}"));
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
            local_id,
            title,
            text,
            submit,
            ..
        }) = self.assistant.cards.get_mut(id).map(|card| card.kind.clone())
        else {
            return;
        };
        let outcome = if approve {
            self.deliver_approved(&local_id, &text, submit)
        } else {
            Err("You declined this message.".to_string())
        };
        let kind = match outcome {
            Ok(target) => CardKind::Sent {
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

    /// Finds the approved agent again by its stable id inside the assistant's
    /// reach, so a card from another session or a workspace since taken out of
    /// scope cannot type into whatever now holds an old panel number, then types
    /// the message.
    fn deliver_approved(&mut self, local_id: &str, text: &str, submit: bool) -> Result<PanelId, String> {
        let Some(assistant) = self.board.assistant_panel() else {
            return Err("The assistant is not running.".to_string());
        };
        let target = self
            .board
            .agent_in_reach(local_id, &self.assistant_reach())
            .ok_or_else(|| "That agent is no longer in the assistant's reach.".to_string())?;
        let now = Instant::now();
        if self.agent_panel_requests.in_flight(target, now) {
            return Err("A message was just sent to that agent. Try again in a moment.".to_string());
        }
        self.board
            .check_agent_can_receive(assistant, target)
            .map_err(|refusal| refusal.message().to_string())?;
        self.send_to_agent(target, text, submit, now)
            .then_some(target)
            .ok_or_else(|| "The agent is no longer available.".to_string())
    }

    /// The cards one under the other, newest first, and what the person did with them.
    pub(super) fn render_cards_list(&mut self, ui: &mut Ui, only_open: bool) {
        let mut actions = Vec::new();
        for card in self.assistant.cards.items.iter().rev() {
            // The feed shows sent messages as agent cards; here only what is open or posted.
            if only_open && matches!(card.kind, CardKind::Sent { .. } | CardKind::Declined { .. }) {
                continue;
            }
            let live = match &card.kind {
                CardKind::Sent { target, .. } => self.board.agent_state(*target),
                _ => None,
            };
            draw_card(ui, card, live, &mut self.assistant.md_cache, &mut actions);
            ui.add_space(8.0);
        }
        self.apply_card_actions(ui.ctx(), actions);
    }

    fn apply_card_actions(&mut self, ctx: &egui::Context, actions: Vec<CardAction>) {
        for action in actions {
            match action {
                CardAction::Approve(id) => self.resolve_approval(id, true),
                CardAction::Decline(id) => self.resolve_approval(id, false),
                CardAction::Dismiss(id) => self.assistant.cards.dismiss(id),
                CardAction::Reveal(target) => self.reveal_selected_panel(ctx, target),
            }
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
                        for card in self.assistant.cards.items.iter().rev() {
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
        self.apply_card_actions(ui.ctx(), actions);
    }
}

/// The question to the person: the full text, who gets it, and the two answers.
fn draw_approval(ui: &mut Ui, id: u64, [title, detail, text]: [&str; 3], submit: bool, actions: &mut Vec<CardAction>) {
    blocks::card(ui, Tone::Attention, |ui| {
        let pill = Some(("Needs your OK", theme::PALETTE_YELLOW()));
        blocks::header(
            ui,
            Icon::Send,
            Tone::Attention,
            &format!("Send to {title}?"),
            detail,
            pill,
        );
        ui.add_space(8.0);
        quoted(ui, text, id);
        ui.add_space(4.0);
        let enter = if submit {
            "Enter will be pressed."
        } else {
            "Enter will not be pressed."
        };
        ui.label(RichText::new(enter).size(11.5).color(theme::FG_DIM()));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if blocks::primary(ui, "Send").clicked() {
                actions.push(CardAction::Approve(id));
            }
            if blocks::ghost(ui, "Don't send").clicked() {
                actions.push(CardAction::Decline(id));
            }
        });
    });
}

fn draw_card(
    ui: &mut Ui,
    card: &Card,
    live: Option<AgentState>,
    markdown_cache: &mut egui_commonmark::CommonMarkCache,
    actions: &mut Vec<CardAction>,
) {
    match &card.kind {
        CardKind::Approval {
            title,
            detail,
            text,
            submit,
            ..
        } => draw_approval(ui, card.id, [title, detail, text], *submit, actions),
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
///
/// The whole text is shown, scrolling when long: the person must see everything
/// that will be typed before approving it.
fn quoted(ui: &mut Ui, text: &str, id: u64) {
    egui::Frame::new()
        .fill(theme::PANEL_BG())
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ScrollArea::vertical()
                .id_salt(("approval_text", id))
                .max_height(140.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(RichText::new(text).size(12.5).color(theme::FG_SOFT()));
                });
        });
}

#[cfg(test)]
mod tests;
