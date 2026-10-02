//! The assistant's conversation as a feed instead of a terminal (desk mode).
//!
//! What was said goes in bubbles, what the assistant did in small activity pills,
//! the plan as a card with a progress bar, every agent as a card with its state,
//! and an agent that asks the person something as a card with the answers as
//! buttons. The terminal stays one click away for the raw view.

use std::time::Instant;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::{AgentState, StepStatus};

use super::super::{blocks, icons, num, plan};
use super::HorizonApp;
use super::desk_bar::paint::{elide, paint_click};
use crate::theme;

const MAX_ENTRIES: usize = 60;
const BUBBLE_SHARE: f32 = 0.74;
const AGENT_COLUMNS: usize = 3;

pub(in crate::app::assistant) enum Entry {
    /// What the person said or typed.
    You { text: String, voice: bool },
    /// What the assistant said.
    Said(String),
    /// Something the assistant did with its tools.
    Did(String),
    /// A plan the agent proposes.
    Plan(String),
}

#[derive(Default)]
pub(in crate::app::assistant) struct Feed {
    entries: Vec<Entry>,
}

impl Feed {
    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }

    pub(in crate::app::assistant) fn you(&mut self, text: &str, voice: bool) {
        let text = text.trim();
        if !text.is_empty() {
            self.push(Entry::You {
                text: text.to_string(),
                voice,
            });
        }
    }

    pub(in crate::app::assistant) fn said(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.push(Entry::Said(text.to_string()));
        }
    }

    pub(in crate::app::assistant) fn plan(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.push(Entry::Plan(text.to_string()));
        }
    }

    pub(in crate::app::assistant) fn did(&mut self, text: impl Into<String>) {
        let text = text.into();
        if matches!(self.entries.last(), Some(Entry::Did(last)) if *last == text) {
            return;
        }
        self.push(Entry::Did(text));
    }

    /// The newest thing the assistant did, if the last entry is one.
    pub(in crate::app::assistant) fn latest_did(&self) -> Option<&str> {
        self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Did(text) => Some(text.as_str()),
            _ => None,
        })
    }

    pub(in crate::app::assistant) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// One ask and everything that followed it, until the next ask.
#[derive(Clone, Default)]
pub(in crate::app::assistant) struct Turn {
    pub(in crate::app::assistant) asked: Option<String>,
    pub(in crate::app::assistant) voice: bool,
    pub(in crate::app::assistant) steps: Vec<String>,
    pub(in crate::app::assistant) replies: Vec<String>,
    /// The newest plan the agent proposed in this turn.
    pub(in crate::app::assistant) plan: Option<String>,
}

impl Feed {
    /// The conversation grouped by what the person asked.
    pub(in crate::app::assistant) fn turns(&self) -> Vec<Turn> {
        let mut turns: Vec<Turn> = Vec::new();
        for entry in &self.entries {
            if let Entry::You { text, voice } = entry {
                turns.push(Turn {
                    asked: Some(text.clone()),
                    voice: *voice,
                    ..Turn::default()
                });
                continue;
            }
            if turns.is_empty() {
                turns.push(Turn::default());
            }
            if let Some(turn) = turns.last_mut() {
                match entry {
                    Entry::Said(text) => turn.replies.push(text.clone()),
                    Entry::Did(text) => turn.steps.push(text.clone()),
                    Entry::Plan(text) => turn.plan = Some(text.clone()),
                    Entry::You { .. } => {}
                }
            }
        }
        turns
    }
}

/// One agent as the feed shows it.
pub(super) struct AgentRow {
    pub(super) id: PanelId,
    pub(super) title: String,
    kind: String,
    pub(super) workspace: usize,
    pub(super) workspace_name: String,
    pub(super) state: AgentState,
    pub(super) last: String,
}

enum FeedAction {
    Answer(PanelId, bool),
    Go(usize),
}

impl HorizonApp {
    /// A line saying what the assistant is up to, for the header.
    pub(super) fn feed_status(&self) -> (String, Color32) {
        let rows = self.feed_agents();
        let needs = rows.iter().filter(|row| row.state == AgentState::NeedsInput).count();
        let working = rows.iter().filter(|row| row.state == AgentState::Working).count();
        let demo = self.assistant.demo.as_ref();
        if demo.is_some_and(super::demo::Demo::voice_active) {
            return ("Listening and speaking".to_string(), theme::ACCENT());
        }
        if needs > 0 {
            return (
                format!("{needs} waiting for you, {working} working"),
                theme::PALETTE_YELLOW(),
            );
        }
        if working > 0 {
            return (format!("{working} agents working"), theme::PALETTE_YELLOW());
        }
        (self.scope_label(), theme::FG_DIM())
    }

    pub(super) fn feed_agents(&self) -> Vec<AgentRow> {
        let reach = self.assistant_reach();
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind.is_agent() && !panel.is_assistant() && reach.contains(panel.workspace_id))
            .filter_map(|panel| {
                let state = self.board.agent_state(panel.id)?;
                let workspace = self
                    .board
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == panel.workspace_id)?;
                let last = self
                    .board
                    .agent_output(panel.id, 8)
                    .and_then(|(text, _)| {
                        text.lines()
                            .rev()
                            .map(str::trim)
                            .find(|line| !line.is_empty() && *line != ">")
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                Some(AgentRow {
                    id: panel.id,
                    title: panel.display_title().into_owned(),
                    kind: horizon_core::agent_definition(panel.kind)
                        .map_or("agent", |agent| agent.display_name)
                        .to_string(),
                    workspace,
                    workspace_name: self.board.workspaces[workspace].name.clone(),
                    state,
                    last,
                })
            })
            .collect()
    }

    /// The feed on a recessed panel, `height` tall.
    pub(super) fn feed_view(&mut self, ui: &mut Ui, height: f32, with_plan: bool) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(14), theme::BG());
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(14),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let inner = rect.shrink2(vec2(14.0, 12.0));
        let agents = self.feed_agents();
        self.update_assistant_cards();
        // The state sits below the chat and does not scroll. Measure it with an invisible
        // pass so the chat gets exactly the height that is left.
        // The cards view keeps only a question pinned: the plan and the agents are in the card.
        let cards = self.assistant.summon.feed_style == super::turns::FeedStyle::Cards;
        let pinned = |this: &Self, ui: &mut Ui, actions: &mut Vec<FeedAction>| {
            // The dock shows questions itself: in a card, on a board or on the panel that asked.
            if !super::dock_enabled() {
                this.draw_attention(ui, &agents, actions);
            }
            if cards {
                return;
            }
            if with_plan && !this.assistant.plan.is_empty() {
                this.draw_plan_card(ui);
                ui.add_space(8.0);
            }
            Self::draw_agents(ui, &agents, actions);
        };
        let mut probe = ui.new_child(egui::UiBuilder::new().max_rect(inner).invisible());
        pinned(self, &mut probe, &mut Vec::new());
        let pinned_height = probe
            .min_rect()
            .height()
            .min((inner.height() - 90.0).max(inner.height() * 0.4));
        let chat = Rect::from_min_size(
            inner.min,
            vec2(inner.width(), (inner.height() - pinned_height).max(60.0)),
        );
        let mut chat_ui = ui.new_child(egui::UiBuilder::new().max_rect(chat));
        ScrollArea::vertical()
            .id_salt("desk_feed")
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(&mut chat_ui, |ui| {
                ui.set_width(ui.available_width());
                match self.assistant.summon.feed_style {
                    super::turns::FeedStyle::Chat => self.draw_entries(ui),
                    super::turns::FeedStyle::Cards => self.draw_turn_cards(ui),
                }
                self.draw_note_cards(ui);
            });
        let mut actions = Vec::new();
        let state = Rect::from_min_max(pos2(inner.left(), inner.bottom() - pinned_height), inner.right_bottom());
        let mut state_ui = ui.new_child(egui::UiBuilder::new().max_rect(state));
        pinned(self, &mut state_ui, &mut actions);
        for action in actions {
            self.apply_feed_action(&action);
        }
    }

    fn apply_feed_action(&mut self, action: &FeedAction) {
        match *action {
            FeedAction::Go(index) => {
                if let Some(desk) = self.assistant.desk.as_ref() {
                    desk.switch(index);
                }
            }
            FeedAction::Answer(id, allow) => self.answer_agent(id, allow),
        }
    }

    /// The person's own answer to an agent that asked: typed into it as they would.
    pub(in crate::app::assistant) fn answer_agent(&mut self, id: PanelId, allow: bool) {
        let title = self
            .board
            .panel(id)
            .map_or_else(String::new, |panel| panel.display_title().into_owned());
        if self.send_to_agent(id, if allow { "y" } else { "n" }, true, Instant::now()) {
            if allow {
                self.assistant.summon.hub.mark_stopped();
            }
            self.assistant
                .feed
                .did(format!("You {} {title}", if allow { "allowed" } else { "denied" }));
            if let Some(demo) = self.assistant.demo.as_ref() {
                demo.log(&format!("event answered {title}"));
            }
        }
    }

    // ---- conversation ---------------------------------------------------

    fn draw_entries(&self, ui: &mut Ui) {
        let max = (ui.available_width() * BUBBLE_SHARE).max(200.0);
        let mut pills: Vec<&str> = Vec::new();
        for entry in &self.assistant.feed.entries {
            if let Entry::Did(text) = entry {
                pills.push(text);
                continue;
            }
            draw_pills(ui, &mut pills);
            match entry {
                Entry::You { text, voice } => you_bubble(ui, text, *voice, max),
                Entry::Said(text) | Entry::Plan(text) => said_bubble(ui, text, max),
                Entry::Did(_) => {}
            }
            ui.add_space(8.0);
        }
        draw_pills(ui, &mut pills);
        self.draw_agent_tail(ui, max);
    }

    /// With a real agent (Claude, Codex, ...) nobody reports what it says, so its latest terminal
    /// output stands in for its replies, until the feed has replies of its own.
    fn draw_agent_tail(&self, ui: &mut Ui, max: f32) {
        // A scripted demo's stand-in agent has nothing worth showing; its replies come as events.
        if self.assistant.demo.is_some()
            || self
                .assistant
                .feed
                .entries
                .iter()
                .any(|entry| matches!(entry, Entry::Said(_)))
        {
            return;
        }
        let Some(id) = self.board.assistant_panel() else {
            return;
        };
        let Some(panel) = self.board.panel(id) else {
            return;
        };
        let Some((text, _)) = self.board.agent_output(id, 40) else {
            return;
        };
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        if tail.is_empty() {
            return;
        }
        let name = horizon_core::agent_definition(panel.kind).map_or("Agent", |agent| agent.display_name);
        ui.label(RichText::new(format!("{name}, live")).size(10.5).color(theme::FG_DIM()));
        ui.add_space(2.0);
        Frame::new()
            .fill(theme::BG_ELEVATED())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_max_width(max.max(ui.available_width() * 0.9));
                ui.label(
                    RichText::new(tail)
                        .font(FontId::monospace(11.5))
                        .color(theme::FG_SOFT()),
                );
            });
        ui.add_space(8.0);
    }

    /// The plan as one slim card: label, progress and a chip for each step.
    fn draw_plan_card(&self, ui: &mut Ui) {
        let steps = &self.assistant.plan;
        let done = steps.iter().filter(|step| step.status == StepStatus::Done).count();
        Frame::new()
            .fill(theme::BG_ELEVATED())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Plan").size(13.0).strong().color(theme::FG()));
                    ui.label(RichText::new(plan::progress(steps)).size(11.5).color(theme::FG_DIM()));
                });
                ui.add_space(3.0);
                progress_bar(ui, done, steps.len());
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for step in steps {
                        step_chip(ui, step);
                    }
                });
                if steps.iter().any(|step| step.status == StepStatus::Running) {
                    ui.ctx().request_repaint();
                }
            });
    }

    // ---- agents ---------------------------------------------------------

    /// Agents asking the person something, each as a card with the answers.
    fn draw_attention(&self, ui: &mut Ui, agents: &[AgentRow], actions: &mut Vec<FeedAction>) {
        for row in agents.iter().filter(|row| row.state == AgentState::NeedsInput) {
            self.attention_card(ui, row, actions);
            ui.add_space(8.0);
        }
    }

    /// The agents that are not asking, three to a row.
    fn draw_agents(ui: &mut Ui, agents: &[AgentRow], actions: &mut Vec<FeedAction>) {
        let others: Vec<&AgentRow> = agents
            .iter()
            .filter(|row| row.state != AgentState::NeedsInput)
            .collect();
        if others.is_empty() {
            return;
        }
        let gap = 10.0;
        let columns = AGENT_COLUMNS.min(others.len()).max(1);
        let width = (ui.available_width() - gap * num::count(columns - 1)) / num::count(columns);
        let rows: Vec<&[&AgentRow]> = others.chunks(AGENT_COLUMNS).collect();
        for chunk in rows {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for row in chunk {
                    agent_card(ui, row, width, actions);
                }
            });
            ui.add_space(gap);
        }
    }

    /// An agent that is asking the person something: the question and the answers.
    fn attention_card(&self, ui: &mut Ui, row: &AgentRow, actions: &mut Vec<FeedAction>) {
        let color = theme::PALETTE_YELLOW();
        let pressed = self.assistant.demo.as_ref().and_then(super::demo::Demo::press_progress);
        let frame = Frame::new()
            .fill(theme::blend(theme::BG_ELEVATED(), color, 0.09))
            .stroke(Stroke::new(1.2, color.gamma_multiply(0.55)))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(12));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icons::tile(ui, icons::Icon::Shield, color, 34.0);
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} is asking", row.title))
                                .size(14.0)
                                .strong()
                                .color(theme::FG()),
                        )
                        .truncate(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} - workspace {} {}",
                            row.kind,
                            row.workspace + 1,
                            row.workspace_name
                        ))
                        .size(11.5)
                        .color(theme::FG_DIM()),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| pill(ui, "Needs you", color));
            });
            ui.add_space(10.0);
            Frame::new()
                .fill(theme::PANEL_BG())
                .corner_radius(CornerRadius::same(9))
                .inner_margin(Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(elide(&row.last, 110))
                            .font(FontId::monospace(12.5))
                            .color(theme::FG()),
                    );
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let allow = blocks::primary(ui, "Allow");
                if allow.clicked() {
                    actions.push(FeedAction::Answer(row.id, true));
                }
                if let Some(progress) = pressed {
                    paint_click(ui, allow.rect.center(), progress);
                }
                if blocks::ghost(ui, "Deny").clicked() {
                    actions.push(FeedAction::Answer(row.id, false));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if blocks::ghost(ui, &format!("Open workspace {}", row.workspace + 1)).clicked() {
                        actions.push(FeedAction::Go(row.workspace));
                    }
                });
            });
        });
        ui.ctx().request_repaint();
    }

    /// Approvals the assistant queued and notes it posted: the existing cards.
    fn draw_note_cards(&mut self, ui: &mut Ui) {
        if self.assistant.cards.is_empty() {
            return;
        }
        ui.add_space(4.0);
        self.render_cards_list(ui, true);
    }
}

// ---- painting -----------------------------------------------------------

fn you_bubble(ui: &mut Ui, text: &str, voice: bool, max: f32) {
    ui.with_layout(Layout::top_down(Align::Max), |ui| {
        ui.label(
            RichText::new(if voice { "You, by voice" } else { "You" })
                .size(10.5)
                .color(theme::FG_DIM()),
        );
        ui.add_space(2.0);
        Frame::new()
            .fill(theme::blend(theme::BG_ELEVATED(), theme::ACCENT(), 0.32))
            .stroke(Stroke::new(1.0, theme::ACCENT().gamma_multiply(0.5)))
            .corner_radius(CornerRadius {
                nw: 16,
                ne: 16,
                sw: 16,
                se: 4,
            })
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_max_width(max);
                ui.label(RichText::new(text).size(14.0).color(theme::FG()));
            });
    });
}

fn said_bubble(ui: &mut Ui, text: &str, max: f32) {
    ui.horizontal_top(|ui| {
        let (mark, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        icons::paint_mark(ui.painter(), mark);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Assistant").size(10.5).color(theme::FG_DIM()));
            ui.add_space(2.0);
            Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                .corner_radius(CornerRadius {
                    nw: 4,
                    ne: 16,
                    sw: 16,
                    se: 16,
                })
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_max_width(max);
                    ui.label(RichText::new(text).size(14.0).color(theme::FG()));
                });
        });
    });
}

/// Consecutive activity as one wrapped row of pills, then empties the list.
fn draw_pills(ui: &mut Ui, pills: &mut Vec<&str>) {
    if pills.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.add_space(34.0);
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for text in pills.iter() {
            activity_pill(ui, text);
        }
    });
    ui.add_space(8.0);
    pills.clear();
}

fn activity_pill(ui: &mut Ui, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), theme::FG_SOFT());
    let size = vec2(galley.size().x + 30.0, 24.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    ui.painter()
        .circle_filled(rect.left_center() + vec2(12.0, 0.0), 3.0, theme::ACCENT());
    ui.painter().galley(
        rect.left_center() + vec2(22.0, -galley.size().y / 2.0),
        galley,
        theme::FG_SOFT(),
    );
}

fn pill(ui: &mut Ui, text: &str, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(20.0, 9.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        color.gamma_multiply(0.16),
        Stroke::new(1.0, color.gamma_multiply(0.45)),
        StrokeKind::Inside,
    );
    ui.painter().galley(rect.left_top() + vec2(10.0, 4.5), galley, color);
}

fn step_chip(ui: &mut Ui, step: &horizon_core::browser::manifest::agent_panels::PlanStep) {
    let title = elide(&step.title, 34);
    let galley = ui
        .painter()
        .layout_no_wrap(title, FontId::proportional(12.5), theme::FG());
    let detail = step.detail.as_ref().map(|detail| {
        ui.painter()
            .layout_no_wrap(detail.clone(), FontId::monospace(10.5), theme::FG_DIM())
    });
    let width = 12.0 + plan::MARKER + 8.0 + galley.size().x + detail.as_ref().map_or(0.0, |d| d.size().x + 10.0) + 12.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
    let running = step.status == StepStatus::Running;
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        if running {
            theme::blend(theme::PANEL_BG(), theme::ACCENT(), 0.14)
        } else {
            theme::PANEL_BG()
        },
        Stroke::new(
            1.0,
            if running {
                theme::ACCENT().gamma_multiply(0.5)
            } else {
                theme::BORDER_SUBTLE()
            },
        ),
        StrokeKind::Inside,
    );
    plan::paint_marker(
        ui,
        Rect::from_center_size(
            rect.left_center() + vec2(12.0 + plan::MARKER / 2.0, 0.0),
            vec2(plan::MARKER, plan::MARKER),
        ),
        step.status,
    );
    let dim = step.status == StepStatus::Pending;
    let mut x = rect.left() + 12.0 + plan::MARKER + 8.0;
    let color = if dim { theme::FG_DIM() } else { theme::FG() };
    let height = galley.size().y;
    let right = x + galley.size().x + 10.0;
    ui.painter()
        .galley(pos2(x, rect.center().y - height / 2.0), galley, color);
    x = right;
    if let Some(detail) = detail {
        let height = detail.size().y;
        ui.painter()
            .galley(pos2(x, rect.center().y - height / 2.0), detail, theme::FG_DIM());
    }
}

fn progress_bar(ui: &mut Ui, done: usize, total: usize) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 5.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), theme::PANEL_BG_ALT());
    if total > 0 {
        let share = num::count(done) / num::count(total);
        let fill = Rect::from_min_size(rect.min, vec2(rect.width() * share, rect.height()));
        ui.painter()
            .rect_filled(fill, CornerRadius::same(3), theme::PALETTE_GREEN());
    }
}

fn agent_card(ui: &mut Ui, row: &AgentRow, width: f32, actions: &mut Vec<FeedAction>) {
    let (label, color) = match row.state {
        AgentState::Working => ("Working", theme::PALETTE_YELLOW()),
        AgentState::Idle => ("Ready", theme::PALETTE_GREEN()),
        AgentState::Starting => ("Starting", theme::FG_DIM()),
        AgentState::Exited => ("Closed", theme::BORDER_STRONG()),
        AgentState::NeedsInput => ("Needs you", theme::PALETTE_RED()),
    };
    let size = vec2(width, 90.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(12),
        if hovered {
            theme::blend(theme::BG_ELEVATED(), color, 0.07)
        } else {
            theme::BG_ELEVATED()
        },
        Stroke::new(
            1.0,
            if hovered {
                color.gamma_multiply(0.6)
            } else {
                theme::BORDER_SUBTLE()
            },
        ),
        StrokeKind::Inside,
    );
    // A coloured spine on the left edge carries the state.
    painter.rect_filled(
        Rect::from_min_size(rect.min + vec2(0.0, 12.0), vec2(3.0, rect.height() - 24.0)),
        CornerRadius::same(2),
        color,
    );
    painter.text(
        rect.min + vec2(16.0, 16.0),
        Align2::LEFT_CENTER,
        elide(&row.title, 18),
        FontId::proportional(13.5),
        theme::FG(),
    );
    painter.text(
        rect.min + vec2(16.0, 33.0),
        Align2::LEFT_CENTER,
        format!("{}  ·  desktop {}", row.kind, row.workspace + 1),
        FontId::proportional(11.0),
        theme::FG_DIM(),
    );
    painter.text(
        rect.min + vec2(16.0, 52.0),
        Align2::LEFT_CENTER,
        elide(&row.last, 30),
        FontId::monospace(11.0),
        theme::FG_SOFT(),
    );
    // State: a pulsing dot and a word; a sweeping bar while working.
    let now = num::seconds(ui);
    let pulse = if row.state == AgentState::Working {
        0.6 + 0.4 * (now * 4.0).sin().abs()
    } else {
        1.0
    };
    let base = rect.left_bottom() + vec2(16.0, -16.0);
    ui.painter().circle_filled(base, 3.5 * pulse + 0.5, color);
    ui.painter().text(
        base + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.5),
        color,
    );
    if row.state == AgentState::Working {
        sweep(
            ui,
            Rect::from_min_size(
                pos2(rect.left() + 14.0, rect.bottom() - 7.0),
                vec2(rect.width() - 28.0, 3.0),
            ),
            now,
            color,
        );
        ui.ctx().request_repaint();
    }
    if response
        .on_hover_text(format!("{} - go to desktop {}", row.workspace_name, row.workspace + 1))
        .clicked()
    {
        actions.push(FeedAction::Go(row.workspace));
    }
}

/// A light band moving along a track.
fn sweep(ui: &Ui, track: Rect, now: f32, color: Color32) {
    ui.painter()
        .rect_filled(track, CornerRadius::same(2), color.gamma_multiply(0.14));
    let band = track.width() * 0.3;
    let phase = (now * 0.9).fract();
    let left = track.left() - band + (track.width() + band) * phase;
    let clipped = Rect::from_min_max(
        pos2(left.max(track.left()), track.top()),
        pos2((left + band).min(track.right()), track.bottom()),
    );
    if clipped.width() > 0.0 {
        ui.painter().rect_filled(clipped, CornerRadius::same(2), color);
    }
}
