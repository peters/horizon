//! Design 3, the lens: the assistant lives on the canvas, next to the work.
//!
//! Mini: nothing stacks. Each agent's question is a bubble on the panel that asked it, with Allow and
//! Deny right there, and each panel wears a small state chip. A slim pill at the bottom counts the
//! questions, says how many are out of sight, and jumps to the next one.
//! Expanded: a sheet on the right, and the canvas becomes a lens: the workspace in scope is outlined with
//! a note on each of its panels, the rest dims, and a click on a dimmed panel moves the lens there.

use egui::{
    Align2, Area, Color32, Context, CornerRadius, FontId, Frame, Id, Margin, Order, Rect, RichText, Sense, Shadow,
    Stroke, StrokeKind, Ui, pos2, vec2,
};
use horizon_core::PanelId;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::super::{icons, num};
use super::super::desk_bar::paint::elide;
use super::super::mini::{MiniAction, round_button, small_button};
use super::super::{Action, HorizonApp, demo};
use super::concierge::risk_chip;
use super::inbox::Ask;
use crate::theme;

const PILL_WIDTH: f32 = 520.0;
const PILL_HEIGHT: f32 = 58.0;
const SHEET_WIDTH: f32 = 470.0;
const BUBBLE_WIDTH: f32 = 360.0;
const BUBBLE_HEIGHT: f32 = 100.0;

impl HorizonApp {
    // ---- mini --------------------------------------------------------------------------

    pub(super) fn lens_mini(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let asks = self.inbox();
        let agents = self.feed_agents();
        let mut answers: Vec<(PanelId, bool)> = Vec::new();
        let mut reveal = None;
        // On the panels: a state chip for each agent, a bubble for each question.
        let mut elsewhere = 0;
        for ask in &asks {
            if !self.panel_screen_rects.contains_key(&ask.id) {
                elsewhere += 1;
            }
        }
        for agent in &agents {
            let Some(rect) = self
                .panel_screen_rects
                .get(&agent.id)
                .copied()
                .filter(|rect| canvas.intersects(*rect))
            else {
                continue;
            };
            Self::paint_panel_chip(ctx, agent.id, rect, agent.state);
            if let Some(ask) = asks.iter().find(|ask| ask.id == agent.id) {
                self.paint_bubble(ctx, canvas, rect, ask, &mut answers);
            }
        }
        // At the bottom: the count and the way to the next one.
        let mut actions: Vec<MiniAction> = Vec::new();
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(canvas.center().x, canvas.bottom() - 24.0))
            .show(ctx, |ui| {
                let (pill, _) =
                    ui.allocate_exact_size(vec2(PILL_WIDTH.min(canvas.width() - 48.0), PILL_HEIGHT), Sense::hover());
                reveal = self.paint_lens_pill(ui, pill, &asks, elsewhere, &mut actions);
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
        for (id, allow) in answers {
            self.answer_agent(id, allow);
        }
        if let Some(id) = reveal {
            self.reveal_selected_panel(ctx, id);
        }
    }

    fn paint_lens_pill(
        &mut self,
        ui: &mut Ui,
        pill: Rect,
        asks: &[Ask],
        elsewhere: usize,
        actions: &mut Vec<MiniAction>,
    ) -> Option<PanelId> {
        let radius = CornerRadius::same(29);
        let agents = self.feed_agents();
        let now = num::seconds(ui);
        let listening = self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening);
        let speaking = self.speaking_now();
        ui.painter().add(
            Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(125),
            }
            .as_shape(pill, radius),
        );
        ui.painter().rect_filled(pill, radius, theme::BG_ELEVATED());
        ui.painter().rect_stroke(
            pill,
            radius,
            Stroke::new(
                1.2,
                if asks.is_empty() {
                    theme::ACCENT().gamma_multiply(0.55)
                } else {
                    theme::PALETTE_YELLOW()
                },
            ),
            StrokeKind::Inside,
        );
        let centre = pos2(pill.left() + 34.0, pill.center().y);
        self.paint_orb(ui, centre, 22.0, &agents, now);
        if ui
            .interact(
                Rect::from_center_size(centre, vec2(48.0, 48.0)),
                Id::new("lens_orb"),
                Sense::click(),
            )
            .on_hover_text("Open the lens")
            .clicked()
        {
            actions.push(MiniAction::Expand);
        }
        let mic = Rect::from_center_size(pos2(pill.right() - 30.0, pill.center().y), vec2(36.0, 36.0));
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
        let mut reveal = None;
        let text = if asks.is_empty() {
            if let Some(line) = self.assistant.demo.as_ref().and_then(demo::Demo::speaking_line) {
                elide(line, 52)
            } else if self.assistant_busy() {
                "Working. Questions will appear on the panels.".to_string()
            } else {
                "Ask anything".to_string()
            }
        } else if elsewhere > 0 {
            format!("{} need you  -  {elsewhere} out of sight", asks.len())
        } else {
            format!("{} need you, right on the panels", asks.len())
        };
        ui.painter().text(
            pos2(pill.left() + 70.0, pill.center().y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(13.5),
            theme::FG(),
        );
        if let Some(next) = asks.iter().next().filter(|_| !asks.is_empty()) {
            let button = Rect::from_center_size(pos2(mic.left() - 52.0, pill.center().y), vec2(88.0, 30.0));
            if small_button(ui, button, "Next", true).clicked() {
                reveal = Some(next.id);
            }
        }
        ui.ctx().request_repaint();
        reveal
    }

    /// A chip on a panel's top-right corner: what its agent is doing.
    fn paint_panel_chip(ctx: &Context, id: PanelId, rect: Rect, state: AgentState) {
        let (label, color) = match state {
            AgentState::Working => ("Working", theme::PALETTE_YELLOW()),
            AgentState::NeedsInput => ("Needs you", theme::PALETTE_RED()),
            AgentState::Idle => ("Ready", theme::PALETTE_GREEN()),
            AgentState::Starting | AgentState::Exited => return,
        };
        Area::new(Id::new(("lens_chip", id.0)))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(pos2(rect.right() - 74.0, rect.top() + 6.0))
            .interactable(false)
            .show(ctx, |ui| {
                let galley = ui
                    .painter()
                    .layout_no_wrap(label.to_string(), FontId::proportional(11.5), color);
                let (chip, _) = ui.allocate_exact_size(galley.size() + vec2(28.0, 10.0), Sense::hover());
                ui.painter().rect(
                    chip,
                    CornerRadius::same(99),
                    theme::BG_ELEVATED(),
                    Stroke::new(1.0, color.gamma_multiply(0.7)),
                    StrokeKind::Inside,
                );
                ui.painter()
                    .circle_filled(chip.left_center() + vec2(11.0, 0.0), 3.5, color);
                ui.painter()
                    .galley(chip.left_center() + vec2(20.0, -galley.size().y / 2.0), galley, color);
            });
    }

    /// The question, on the panel that asked it.
    fn paint_bubble(&self, ctx: &Context, canvas: Rect, panel: Rect, ask: &Ask, answers: &mut Vec<(PanelId, bool)>) {
        let at = pos2(
            (panel.left() + 24.0).clamp(
                canvas.left() + 8.0,
                (canvas.right() - BUBBLE_WIDTH - 8.0).max(canvas.left()),
            ),
            (panel.top() + 64.0).clamp(
                canvas.top() + 8.0,
                (canvas.bottom() - BUBBLE_HEIGHT - 90.0).max(canvas.top()),
            ),
        );
        Area::new(Id::new(("lens_bubble", ask.id.0)))
            .order(Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(BUBBLE_WIDTH, BUBBLE_HEIGHT), Sense::hover());
                let color = ask.risk.color();
                ui.painter().add(
                    Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    }
                    .as_shape(rect, CornerRadius::same(16)),
                );
                ui.painter().rect(
                    rect,
                    CornerRadius::same(16),
                    theme::BG_ELEVATED(),
                    Stroke::new(1.4, color.gamma_multiply(0.8)),
                    StrokeKind::Inside,
                );
                let after = risk_chip(ui, rect.left_top() + vec2(14.0, 20.0), ask.risk);
                ui.painter().text(
                    pos2(after + 8.0, rect.top() + 20.0),
                    Align2::LEFT_CENTER,
                    elide(&ask.agent, 24),
                    FontId::proportional(12.0),
                    theme::FG_DIM(),
                );
                ui.painter().text(
                    rect.left_top() + vec2(14.0, 48.0),
                    Align2::LEFT_CENTER,
                    elide(&ask.text, 48),
                    FontId::proportional(13.5),
                    theme::FG(),
                );
                let allow = Rect::from_center_size(pos2(rect.right() - 108.0, rect.bottom() - 22.0), vec2(66.0, 28.0));
                let deny = Rect::from_center_size(pos2(rect.right() - 40.0, rect.bottom() - 22.0), vec2(62.0, 28.0));
                if small_button(ui, allow, "Allow", true).clicked() {
                    answers.push((ask.id, true));
                }
                if let Some(progress) = self.assistant.demo.as_ref().and_then(demo::Demo::press_progress) {
                    super::super::desk_bar::paint::paint_click(ui, allow.center(), progress);
                }
                if small_button(ui, deny, "Deny", false).clicked() {
                    answers.push((ask.id, false));
                }
            });
    }

    // ---- expanded ----------------------------------------------------------------------

    pub(super) fn lens_panel(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        self.paint_lens_over_canvas(ctx, canvas);
        let width = SHEET_WIDTH.min(canvas.width() - 32.0);
        let height = canvas.height() - 32.0;
        let tiles = self.dock_tiles();
        let mut action = None;
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(pos2(canvas.right() - 16.0, canvas.top() + 16.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(24))
                    .inner_margin(Margin::same(16))
                    .shadow(Shadow {
                        offset: [-8, 12],
                        blur: 50,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);
                        ui.set_height(height - 32.0);
                        self.dock_header(ui, &mut action);
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("THE LENS")
                                .size(10.0)
                                .extra_letter_spacing(0.9)
                                .color(theme::FG_DIM()),
                        );
                        ui.add_space(4.0);
                        self.scope_strip(ui);
                        ui.add_space(8.0);
                        self.history_tabs(ui);
                        ui.add_space(8.0);
                        let body = (ui.available_height() - 62.0 - 8.0).max(120.0);
                        self.conversation(ui, body, false);
                        ui.add_space(8.0);
                        self.desk_prompt(ui, &mut action);
                    });
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        self.apply_sheet_action(ctx, action);
    }

    /// Two tabs: the threads of the workspace in the lens, and the global ones.
    fn history_tabs(&mut self, ui: &mut Ui) {
        let scope = self.scope_label();
        let here = !self.assistant.scope.is_all();
        let groups: Vec<(String, Vec<String>)> = self
            .assistant
            .threads
            .by_space()
            .into_iter()
            .map(|(space, threads)| {
                (
                    space.to_string(),
                    threads.into_iter().map(|t| t.title.clone()).collect(),
                )
            })
            .collect();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            ui.label(
                RichText::new("HISTORY")
                    .size(10.0)
                    .extra_letter_spacing(0.9)
                    .color(theme::FG_DIM()),
            );
            for (space, titles) in &groups {
                let mine = *space == scope && here;
                let global = space == "Everywhere";
                if !(mine || global) {
                    continue;
                }
                for title in titles.iter().take(2) {
                    let text = format!("{}  {}", if global { "Everywhere" } else { "Here" }, elide(title, 22));
                    let _ = ui.add(
                        egui::Button::new(RichText::new(text).size(11.5).color(theme::FG_SOFT()))
                            .fill(theme::PANEL_BG())
                            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
                            .corner_radius(CornerRadius::same(99)),
                    );
                }
            }
        });
    }

    /// Dims the panels outside the lens, outlines and annotates the ones inside.
    fn paint_lens_over_canvas(&mut self, ctx: &Context, canvas: Rect) {
        let rects: Vec<(PanelId, Rect)> = self
            .panel_screen_rects
            .iter()
            .map(|(id, rect)| (*id, *rect))
            .filter(|(_, rect)| canvas.intersects(*rect))
            .collect();
        let mut move_lens: Option<String> = None;
        for (id, rect) in rects {
            let Some(panel) = self.board.panel(id) else {
                continue;
            };
            if panel.is_assistant() {
                continue;
            }
            let Some(workspace) = self.board.workspaces.iter().find(|ws| ws.id == panel.workspace_id) else {
                continue;
            };
            let inside = self.assistant.scope.includes(&workspace.local_id);
            let local_id = workspace.local_id.clone();
            let title = panel.display_title().into_owned();
            let note = self
                .feed_agents()
                .into_iter()
                .find(|agent| agent.id == id)
                .map(|agent| elide(&agent.last, 42));
            Area::new(Id::new(("lens_over", id.0)))
                .order(Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (over, response) =
                        ui.allocate_exact_size(rect.size(), if inside { Sense::hover() } else { Sense::click() });
                    if inside {
                        ui.painter().rect_stroke(
                            over,
                            CornerRadius::same(12),
                            Stroke::new(2.0, theme::ACCENT()),
                            StrokeKind::Inside,
                        );
                        if let Some(note) = note.filter(|note| !note.is_empty()) {
                            let galley = ui.painter().layout_no_wrap(
                                format!("{title}: {note}"),
                                FontId::proportional(12.0),
                                theme::FG(),
                            );
                            let chip = Rect::from_min_size(
                                over.left_bottom() + vec2(12.0, -galley.size().y - 22.0),
                                galley.size() + vec2(20.0, 12.0),
                            );
                            ui.painter().rect(
                                chip,
                                CornerRadius::same(10),
                                theme::BG_ELEVATED(),
                                Stroke::new(1.0, theme::ACCENT().gamma_multiply(0.7)),
                                StrokeKind::Inside,
                            );
                            ui.painter().galley(chip.min + vec2(10.0, 6.0), galley, theme::FG());
                        }
                    } else {
                        ui.painter()
                            .rect_filled(over, CornerRadius::same(12), Color32::from_black_alpha(165));
                        if response.hovered() {
                            ui.painter().rect_stroke(
                                over,
                                CornerRadius::same(12),
                                Stroke::new(1.5, theme::ACCENT().gamma_multiply(0.7)),
                                StrokeKind::Inside,
                            );
                        }
                        if response.clicked() {
                            move_lens = Some(local_id.clone());
                        }
                    }
                });
        }
        if let Some(local_id) = move_lens {
            self.assistant.summon.scope_follow = false;
            self.assistant.scope.set_only(&local_id);
        }
    }
}

#[allow(dead_code)]
fn unused(_: Action) {}
