//! The deck (design C): the orb with the news stacked above it as cards.
//!
//! Nothing here asks for a window. A card appears when the agent has finished something, when an agent
//! asks the person a question, and while work is under way; it goes when it is dealt with. Open goes to
//! the cards view, Terminal to the raw terminal.

use egui::{Align2, CornerRadius, FontId, Rect, Stroke, Ui, Vec2, pos2, text::LayoutJob, vec2};
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::desk_bar::paint::{self, elide};
use super::mini::{MiniAction, plate, small_button};
use super::turns::split_reply;
use super::{HorizonApp, demo};
use crate::theme;

pub(super) const CARD_HEIGHT: f32 = 96.0;
pub(super) const GAP: f32 = 8.0;
const MAX_CARDS: usize = 3;

pub(super) enum Toast {
    /// The agent finished an ask: its outcome as a title and a short body.
    Summary { turn: usize, title: String, body: String },
    /// An agent asks the person something.
    Asking {
        id: PanelId,
        title: String,
        question: String,
    },
    /// Work is under way; the latest step.
    Working { step: String },
}

impl HorizonApp {
    pub(super) fn deck_toasts(&self) -> Vec<Toast> {
        let mut toasts = Vec::new();
        let busy = self.assistant_busy();
        let turns = self.assistant.feed.turns();
        if !busy
            && let Some((index, turn)) = turns.iter().enumerate().next_back()
            && index >= self.assistant.summon.deck_dismissed
            && let Some(reply) = turn.replies.last()
        {
            let (title, body) = split_reply(reply);
            let body = if body.is_empty() {
                super::turns::step_counts(&turn.steps)
            } else {
                body
            };
            toasts.push(Toast::Summary {
                turn: index,
                title,
                body,
            });
        }
        if let Some(agent) = self
            .feed_agents()
            .into_iter()
            .find(|agent| agent.state == AgentState::NeedsInput)
        {
            toasts.push(Toast::Asking {
                id: agent.id,
                title: format!("{} asks", agent.title),
                question: agent.last,
            });
        }
        if busy {
            let step = self
                .assistant
                .feed
                .latest_did()
                .map_or_else(|| "Thinking it through".to_string(), str::to_string);
            toasts.push(Toast::Working { step });
        }
        toasts.truncate(MAX_CARDS);
        toasts
    }

    /// The height the deck's cards need above the orb.
    pub(super) fn deck_stack_height(&self) -> f32 {
        let count = self.deck_toasts().len();
        if count == 0 {
            0.0
        } else {
            super::super::num::count(count) * (CARD_HEIGHT + GAP)
        }
    }

    pub(super) fn paint_toasts(&self, ui: &mut Ui, stack: Rect, toasts: &[Toast], actions: &mut Vec<MiniAction>) {
        for (index, toast) in toasts.iter().enumerate() {
            let top = stack.top() + super::super::num::count(index) * (CARD_HEIGHT + GAP);
            let card = Rect::from_min_size(pos2(stack.left(), top), vec2(stack.width() - 4.0, CARD_HEIGHT));
            match toast {
                Toast::Summary { turn, title, body } => {
                    plate(ui, card, theme::PALETTE_GREEN());
                    mark(ui, card, theme::PALETTE_GREEN());
                    text_block(ui, card, title, body);
                    let close = Rect::from_center_size(pos2(card.right() - 24.0, card.top() + 24.0), vec2(30.0, 30.0));
                    let open =
                        Rect::from_center_size(pos2(card.right() - 150.0, card.bottom() - 21.0), vec2(64.0, 28.0));
                    let terminal =
                        Rect::from_center_size(pos2(card.right() - 62.0, card.bottom() - 21.0), vec2(88.0, 28.0));
                    if small_button(ui, open, "Open", true).clicked() {
                        actions.push(MiniAction::Open { terminal: false });
                    }
                    if small_button(ui, terminal, "Terminal", false).clicked() {
                        actions.push(MiniAction::Open { terminal: true });
                    }
                    if small_button(ui, close, "x", false).clicked() {
                        actions.push(MiniAction::Dismiss(*turn));
                    }
                }
                Toast::Asking { id, title, question } => {
                    plate(ui, card, theme::PALETTE_YELLOW());
                    mark(ui, card, theme::PALETTE_YELLOW());
                    text_block(ui, card, title, question);
                    let allow =
                        Rect::from_center_size(pos2(card.right() - 112.0, card.bottom() - 21.0), vec2(64.0, 28.0));
                    let deny =
                        Rect::from_center_size(pos2(card.right() - 44.0, card.bottom() - 21.0), vec2(60.0, 28.0));
                    if small_button(ui, allow, "Allow", true).clicked() {
                        actions.push(MiniAction::Answer(*id, true));
                    }
                    if let Some(progress) = self.assistant.demo.as_ref().and_then(demo::Demo::press_progress) {
                        paint::paint_click(ui, allow.center(), progress);
                    }
                    if small_button(ui, deny, "Deny", false).clicked() {
                        actions.push(MiniAction::Answer(*id, false));
                    }
                }
                Toast::Working { step } => {
                    plate(ui, card, theme::PALETTE_YELLOW());
                    let pulse = 0.6 + 0.4 * (super::super::num::seconds(ui) * 4.0).sin().abs();
                    ui.painter().circle_filled(
                        pos2(card.left() + 26.0, card.top() + 28.0),
                        4.0 + 3.0 * pulse,
                        theme::PALETTE_YELLOW(),
                    );
                    text_block(ui, card, "Working", step);
                    sweep(
                        ui,
                        Rect::from_min_size(
                            pos2(card.left() + 18.0, card.bottom() - 12.0),
                            vec2(card.width() - 36.0, 3.0),
                        ),
                    );
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

/// A dot on the card's left edge.
fn mark(ui: &Ui, card: Rect, color: egui::Color32) {
    let centre = pos2(card.left() + 26.0, card.top() + 28.0);
    ui.painter().circle_filled(centre, 8.0, color.gamma_multiply(0.22));
    ui.painter().circle_stroke(centre, 8.0, Stroke::new(1.4, color));
}

/// A title and up to two lines of body, to the right of the dot and clear of the buttons.
fn text_block(ui: &Ui, card: Rect, title: &str, body: &str) {
    ui.painter().text(
        card.left_top() + vec2(48.0, 27.0),
        Align2::LEFT_CENTER,
        elide(
            title,
            super::super::num::index((card.width() - 130.0) / 7.4).clamp(16, 80),
        ),
        FontId::proportional(14.5),
        theme::FG(),
    );
    let mut job = LayoutJob::simple(
        body.to_string(),
        FontId::proportional(12.0),
        theme::FG_SOFT(),
        card.width() - 70.0 - 56.0,
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    ui.painter()
        .galley(card.left_top() + Vec2::new(48.0, 41.0), galley, theme::FG_SOFT());
}

/// A light band travelling along a thin track.
fn sweep(ui: &Ui, track: Rect) {
    let color = theme::PALETTE_YELLOW();
    ui.painter()
        .rect_filled(track, CornerRadius::same(2), color.gamma_multiply(0.15));
    let band = track.width() * 0.25;
    let phase = (super::super::num::seconds(ui) * 0.9).fract();
    let left = track.left() - band + (track.width() + band) * phase;
    let clipped = Rect::from_min_max(
        pos2(left.max(track.left()), track.top()),
        pos2((left + band).min(track.right()), track.bottom()),
    );
    if clipped.width() > 0.0 {
        ui.painter().rect_filled(clipped, CornerRadius::same(2), color);
    }
}
