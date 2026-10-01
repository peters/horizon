//! Design 2, mission control: the assistant delegates, and you steer a fleet.
//!
//! Mini: a status strip. It counts what is working, what needs you and what is done, shows the next
//! question in a line, and has one button to review them all. Nothing stacks and nothing asks
//! one at a time. Expanded: a board of the delegated work in three columns. The questions are one column,
//! ordered by risk, with a button for the harmless ones; the working agents and the finished ones
//! have their own. A conversation can open beside it.

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Rect, RichText,
    ScrollArea, Sense, Shadow, Stroke, StrokeKind, Ui, pos2, vec2,
};
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::super::{blocks, icons, num};
use super::super::desk_bar::paint::elide;
use super::super::mini::{MiniAction, chevron_button, round_button, small_button};
use super::super::{HorizonApp, demo};
use super::concierge::{count_chip, risk_chip};
use super::inbox::{Ask, Risk};
use crate::theme;

const STRIP_HEIGHT: f32 = 64.0;
const STRIP_WIDTH: f32 = 860.0;
const MARGIN: f32 = 22.0;
const CHAT_WIDTH: f32 = 400.0;

/// What a press on the board asked for.
enum Pressed {
    Answer(PanelId, bool),
    AllowLow,
    Reveal(PanelId),
}

impl HorizonApp {
    // ---- mini --------------------------------------------------------------------------

    pub(super) fn mission_mini(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let width = STRIP_WIDTH.min(canvas.width() - 48.0).max(480.0);
        let mut actions: Vec<MiniAction> = Vec::new();
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(canvas.center().x, canvas.bottom() - 24.0))
            .show(ctx, |ui| {
                let (strip, _) = ui.allocate_exact_size(vec2(width, STRIP_HEIGHT), Sense::hover());
                self.paint_strip(ui, strip, &mut actions);
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
    }

    fn paint_strip(&mut self, ui: &mut Ui, strip: Rect, actions: &mut Vec<MiniAction>) {
        let radius = CornerRadius::same(32);
        let agents = self.feed_agents();
        let asks = self.inbox();
        let now = num::seconds(ui);
        let listening = self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening);
        let speaking = self.speaking_now();
        let count = |state: AgentState| agents.iter().filter(|agent| agent.state == state).count();
        let (working, waiting, ready) = (count(AgentState::Working), asks.len(), count(AgentState::Idle));
        let accent = if waiting > 0 {
            theme::PALETTE_YELLOW()
        } else {
            theme::ACCENT().gamma_multiply(0.55)
        };
        ui.painter().add(
            Shadow {
                offset: [0, 10],
                blur: 34,
                spread: 0,
                color: Color32::from_black_alpha(130),
            }
            .as_shape(strip, radius),
        );
        ui.painter().rect_filled(strip, radius, theme::BG_ELEVATED());
        ui.painter()
            .rect_stroke(strip, radius, Stroke::new(1.3, accent), StrokeKind::Inside);
        let centre = pos2(strip.left() + 38.0, strip.center().y);
        self.paint_orb(ui, centre, 24.0, &agents, now);
        if ui
            .interact(
                Rect::from_center_size(centre, vec2(52.0, 52.0)),
                Id::new("mission_orb"),
                Sense::click(),
            )
            .clicked()
        {
            actions.push(MiniAction::Expand);
        }
        // The counts of the fleet.
        let mut x = strip.left() + 76.0;
        for (number, label, color) in [
            (working, "working", theme::PALETTE_YELLOW()),
            (waiting, "need you", theme::PALETTE_RED()),
            (ready, "ready", theme::PALETTE_GREEN()),
        ] {
            x = stat(ui, pos2(x, strip.center().y), number, label, color) + 8.0;
        }
        // The right side: mic, the button that opens the board.
        let mic = Rect::from_center_size(pos2(strip.right() - 32.0, strip.center().y), vec2(38.0, 38.0));
        if round_button(
            ui,
            mic,
            icons::Icon::Mic,
            "Talk to the assistant",
            listening || speaking,
        )
        .clicked()
        {
            actions.push(MiniAction::Dictate);
        }
        let label = if waiting > 0 {
            format!("Review {waiting}")
        } else {
            "Open board".to_string()
        };
        let button = Rect::from_center_size(pos2(mic.left() - 62.0, strip.center().y), vec2(104.0, 32.0));
        if small_button(ui, button, &label, waiting > 0).clicked() {
            actions.push(MiniAction::Expand);
        }
        let chevron = Rect::from_center_size(pos2(button.left() - 26.0, strip.center().y), vec2(30.0, 30.0));
        let _ = chevron_button;
        let _ = chevron;
        // The next thing, in a line.
        let line = if let Some(ask) = asks.first() {
            format!("Next: {} - {}", ask.agent, ask.text)
        } else if let Some(text) = self.assistant.demo.as_ref().and_then(demo::Demo::speaking_line) {
            text.to_string()
        } else if let Some(step) = self.assistant.feed.latest_did().filter(|_| working > 0) {
            step.to_string()
        } else {
            "All quiet. Ask anything.".to_string()
        };
        let room = (button.left() - 12.0 - x).max(0.0);
        if room > 80.0 {
            ui.painter().text(
                pos2(x + 8.0, strip.center().y),
                Align2::LEFT_CENTER,
                elide(&line, num::index(room / 6.8)),
                FontId::proportional(13.0),
                theme::FG_SOFT(),
            );
        }
        ui.ctx().request_repaint();
    }

    // ---- the board ---------------------------------------------------------------------

    pub(super) fn mission_board(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let outer = canvas.shrink(MARGIN);
        let tiles = self.dock_tiles();
        let asks = self.inbox();
        let agents = self.feed_agents();
        let mut action = None;
        let mut pressed: Vec<Pressed> = Vec::new();
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(outer.min)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(26))
                    .inner_margin(Margin::same(18))
                    .shadow(Shadow {
                        offset: [0, 24],
                        blur: 70,
                        spread: 0,
                        color: Color32::from_black_alpha(160),
                    })
                    .show(ui, |ui| {
                        ui.set_width(outer.width() - 36.0);
                        ui.set_height(outer.height() - 36.0);
                        self.board_header(ui, &mut action);
                        ui.add_space(8.0);
                        self.scope_strip(ui);
                        ui.add_space(10.0);
                        let prompt = 62.0 + 8.0;
                        let body = (ui.available_height() - prompt).max(160.0);
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), body), Sense::hover());
                        let chat = self.assistant.summon.board_chat;
                        let columns_width = rect.width() - if chat { CHAT_WIDTH + 14.0 } else { 0.0 };
                        let columns = Rect::from_min_size(rect.min, vec2(columns_width, rect.height()));
                        let mut cols = ui.new_child(egui::UiBuilder::new().max_rect(columns));
                        board_columns(&mut cols, &asks, &agents, &mut pressed);
                        if chat {
                            let side = Rect::from_min_max(pos2(rect.right() - CHAT_WIDTH, rect.top()), rect.max);
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(side));
                            self.conversation(&mut child, side.height(), false);
                        }
                        ui.add_space(8.0);
                        self.desk_prompt(ui, &mut action);
                    });
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        for press in pressed {
            match press {
                Pressed::Answer(id, allow) => self.answer_agent(id, allow),
                Pressed::AllowLow => self.allow_low_risk(),
                Pressed::Reveal(id) => {
                    self.assistant.summon.expanded = false;
                    self.reveal_selected_panel(ctx, id);
                }
            }
        }
        self.apply_sheet_action(ctx, action);
    }

    fn board_header(&mut self, ui: &mut Ui, action: &mut Option<super::super::Action>) {
        let mission = self
            .assistant
            .feed
            .turns()
            .iter()
            .rev()
            .find_map(|turn| turn.asked.clone())
            .map_or_else(
                || "No mission yet. Say what you want done.".to_string(),
                |asked| elide(&asked, 110),
            );
        let (status, color) = self.feed_status();
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            icons::paint_mark(ui.painter(), mark);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new(mission).size(15.0).strong().color(theme::FG()));
                ui.label(RichText::new(status).size(11.5).color(color));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if super::chevron_down(ui) {
                    *action = Some(super::super::Action::Close);
                }
                ui.add_space(8.0);
                let chat = self.assistant.summon.board_chat;
                if blocks::ghost(ui, if chat { "Hide chat" } else { "Chat" }).clicked() {
                    self.assistant.summon.board_chat = !chat;
                }
            });
        });
    }
}

/// The three columns.
fn board_columns(ui: &mut Ui, asks: &[Ask], agents: &[super::super::feed::AgentRow], pressed: &mut Vec<Pressed>) {
    let gap = 12.0;
    let width = (ui.available_width() - 2.0 * gap) / 3.0;
    let top = ui.max_rect().min;
    let height = ui.max_rect().height();
    let low = asks.iter().filter(|ask| ask.risk == Risk::Low).count();
    for (index, (title, color)) in [
        ("Needs you", theme::PALETTE_RED()),
        ("Working", theme::PALETTE_YELLOW()),
        ("Done", theme::PALETTE_GREEN()),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = Rect::from_min_size(
            pos2(top.x + num::count(index) * (width + gap), top.y),
            vec2(width, height),
        );
        ui.painter()
            .rect_filled(rect, CornerRadius::same(16), theme::PANEL_BG());
        let mut col = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
        col.horizontal(|ui| {
            ui.painter().circle_filled(
                egui::pos2(ui.cursor().left() + 5.0, ui.cursor().center().y + 2.0),
                4.5,
                color,
            );
            ui.add_space(14.0);
            let count = match index {
                0 => asks.len(),
                1 => agents.iter().filter(|agent| agent.state == AgentState::Working).count(),
                _ => agents.iter().filter(|agent| agent.state == AgentState::Idle).count(),
            };
            ui.label(
                RichText::new(format!("{title}  {count}"))
                    .size(13.5)
                    .strong()
                    .color(theme::FG()),
            );
            if index == 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if low >= 2 && blocks::primary(ui, &format!("Allow {low} low-risk")).clicked() {
                        pressed.push(Pressed::AllowLow);
                    }
                });
            }
        });
        col.add_space(8.0);
        ScrollArea::vertical()
            .id_salt(("board_col", index))
            .auto_shrink([false, false])
            .show(&mut col, |ui| match index {
                0 => {
                    for ask in asks {
                        ask_card(ui, ask, pressed);
                    }
                }
                1 => {
                    for agent in agents.iter().filter(|agent| agent.state == AgentState::Working) {
                        agent_card(ui, agent, true, pressed);
                    }
                }
                _ => {
                    for agent in agents.iter().filter(|agent| agent.state == AgentState::Idle) {
                        agent_card(ui, agent, false, pressed);
                    }
                }
            });
    }
}

fn card_frame(ui: &mut Ui, edge: Color32, body: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::BG_ELEVATED())
        .stroke(Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
    ui.add_space(8.0);
}

fn ask_card(ui: &mut Ui, ask: &Ask, pressed: &mut Vec<Pressed>) {
    card_frame(ui, ask.risk.color().gamma_multiply(0.5), |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(90.0, 22.0), Sense::hover());
            risk_chip(ui, rect.left_center(), ask.risk);
            ui.label(
                RichText::new(elide(&ask.workspace, 18))
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
        });
        ui.add_space(4.0);
        ui.label(RichText::new(&ask.agent).size(13.0).strong().color(theme::FG()));
        ui.label(RichText::new(&ask.text).size(12.5).color(theme::FG_SOFT()));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if blocks::primary(ui, "Allow").clicked() {
                pressed.push(Pressed::Answer(ask.id, true));
            }
            if blocks::ghost(ui, "Deny").clicked() {
                pressed.push(Pressed::Answer(ask.id, false));
            }
            if blocks::ghost(ui, "Open").clicked() {
                pressed.push(Pressed::Reveal(ask.id));
            }
        });
    });
}

fn agent_card(ui: &mut Ui, agent: &super::super::feed::AgentRow, working: bool, pressed: &mut Vec<Pressed>) {
    let color = if working {
        theme::PALETTE_YELLOW()
    } else {
        theme::PALETTE_GREEN()
    };
    card_frame(ui, theme::BORDER_SUBTLE(), |ui| {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
            let pulse = if working {
                0.7 + 0.3 * (num::seconds(ui) * 4.0).sin().abs()
            } else {
                1.0
            };
            ui.painter().circle_filled(dot.center(), 4.0 * pulse, color);
            ui.label(RichText::new(&agent.title).size(13.0).strong().color(theme::FG()));
            ui.label(
                RichText::new(elide(&agent.workspace_name, 16))
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
        });
        ui.add_space(3.0);
        ui.label(
            RichText::new(elide(&agent.last, 44))
                .font(FontId::monospace(11.0))
                .color(theme::FG_SOFT()),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if blocks::ghost(ui, "Open").clicked() {
                pressed.push(Pressed::Reveal(agent.id));
            }
        });
    });
    if working {
        ui.ctx().request_repaint();
    }
}

/// A counter of the strip: a dot, a number and a word; returns the right edge.
fn stat(ui: &Ui, left_centre: egui::Pos2, number: usize, label: &str, color: Color32) -> f32 {
    let text = format!("{number} {label}");
    let galley = ui.painter().layout_no_wrap(
        text,
        FontId::proportional(12.5),
        if number > 0 { theme::FG() } else { theme::FG_DIM() },
    );
    let rect = Rect::from_min_size(
        pos2(left_centre.x, left_centre.y - 13.0),
        vec2(galley.size().x + 28.0, 26.0),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        if number > 0 {
            color.gamma_multiply(0.14)
        } else {
            theme::PANEL_BG()
        },
        Stroke::new(
            1.0,
            if number > 0 {
                color.gamma_multiply(0.5)
            } else {
                theme::BORDER_SUBTLE()
            },
        ),
        StrokeKind::Inside,
    );
    ui.painter().circle_filled(
        rect.left_center() + vec2(11.0, 0.0),
        3.5,
        if number > 0 { color } else { theme::BORDER_STRONG() },
    );
    ui.painter().galley(
        rect.left_center() + vec2(21.0, -galley.size().y / 2.0),
        galley,
        if number > 0 { theme::FG() } else { theme::FG_DIM() },
    );
    let _ = count_chip;
    rect.right()
}
