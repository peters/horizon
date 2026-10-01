//! The concierge, expanded: one window you can move and resize, and three ways to have a real agent
//! beside the voice assistant.
//!
//! The voice assistant (Realtime, with every Horizon tool) and the text agent (Claude, Codex or Grok, as
//! a real terminal) share one scope. What the scope chips say is what both of them look at.
//!
//! - **Split**: the conversation on the left, the agent's terminal on the right, a divider to drag.
//! - **Feed**: one feed, two inputs. Say it, or type to the agent; its terminal slides up as a drawer.
//! - **Roster**: a rail lists who is here: the voice assistant, the text agent and the workers in scope.
//!   Pick one and the main area becomes that conversation, or that terminal.

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin, Order, Rect,
    RichText, Sense, Shadow, Stroke, StrokeKind, Ui, pos2, vec2,
};
use horizon_core::PanelId;
use horizon_core::assistant::ASSISTANT_AGENTS;
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::desk_bar::paint::elide;
use super::super::{Action, HorizonApp};
use super::concierge::{approvals_card, mode_chip, static_chip};
use super::inbox::Ask;
use crate::theme;

const MIN_SIZE: [f32; 2] = [760.0, 460.0];
const RAIL: f32 = 230.0;
const EDGE: f32 = 7.0;

/// How the text agent sits beside the voice assistant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Layout3 {
    /// Side by side.
    #[default]
    Split,
    /// One feed, two inputs, a terminal drawer.
    Feed,
    /// A roster of who is here.
    Roster,
}

/// Who the roster's main area shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Pick {
    #[default]
    Voice,
    Text,
    Worker(PanelId),
}

/// The channel the composer sends on, in the feed layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Channel {
    #[default]
    Voice,
    Text,
}

impl HorizonApp {
    pub(super) fn concierge_panel(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let rect = self.sheet_rect(canvas);
        let tiles = self.dock_tiles();
        let asks = self.inbox();
        let mut action = None;
        let mut allow_low = false;
        let mut answers: Vec<(PanelId, bool)> = Vec::new();
        let layout = self.assistant.summon.layout;
        let mut next = rect;
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                // The window chrome first, so the widgets drawn after it win a press.
                let header = Rect::from_min_size(rect.min + vec2(12.0, 0.0), vec2(rect.width() - 24.0, 52.0));
                let drag = ui.interact(header, Id::new("sheet_move"), Sense::drag());
                if drag.hovered() || drag.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab);
                }
                if drag.dragged() {
                    next = rect.translate(drag.drag_delta());
                }
                next = resize(ui, rect, next);
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.2, theme::ACCENT().gamma_multiply(0.5)))
                    .corner_radius(CornerRadius::same(26))
                    .inner_margin(Margin::same(0))
                    .shadow(Shadow {
                        offset: [0, 26],
                        blur: 72,
                        spread: 0,
                        color: Color32::from_black_alpha(165),
                    })
                    .show(ui, |ui| {
                        ui.set_width(rect.width());
                        ui.set_height(rect.height());
                        let whole = ui.max_rect();
                        let rail_on = layout != Layout3::Split || self.assistant.summon.rail_open;
                        let rail = Rect::from_min_max(
                            whole.min,
                            pos2(whole.left() + if rail_on { RAIL } else { 0.0 }, whole.bottom()),
                        );
                        let main = Rect::from_min_max(pos2(rail.right(), whole.top()), whole.max);
                        if rail_on {
                            ui.painter().rect_filled(
                                rail,
                                CornerRadius {
                                    nw: 26,
                                    sw: 26,
                                    ne: 0,
                                    se: 0,
                                },
                                theme::PANEL_BG(),
                            );
                            let mut left =
                                ui.new_child(egui::UiBuilder::new().max_rect(rail.shrink2(vec2(14.0, 16.0))));
                            if layout == Layout3::Roster {
                                self.roster_rail(&mut left);
                            } else if self.assistant.summon.msgs.on {
                                self.message_rail(&mut left);
                            } else {
                                self.thread_rail(&mut left);
                            }
                        }
                        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(main.shrink2(vec2(18.0, 16.0))));
                        match layout {
                            Layout3::Split => {
                                self.layout_split(&mut inner, &asks, &mut action, &mut allow_low, &mut answers);
                            }
                            Layout3::Feed => {
                                self.layout_feed(&mut inner, &asks, &mut action, &mut allow_low, &mut answers);
                            }
                            Layout3::Roster => {
                                self.layout_roster(&mut inner, &asks, &mut action, &mut allow_low, &mut answers);
                            }
                        }
                    });
            });
        self.assistant.summon.sheet = Some(fit(next, canvas));
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        if allow_low {
            self.allow_low_risk();
        }
        for (id, allow) in answers {
            self.answer_agent(id, allow);
        }
        self.apply_sheet_action(ctx, action);
    }

    /// Where the window is: where it was put, a size the script asked for, or the default.
    fn sheet_rect(&mut self, canvas: Rect) -> Rect {
        let default = Rect::from_center_size(
            pos2(canvas.center().x, canvas.bottom() - 24.0 - 380.0),
            vec2((canvas.width() - 48.0).min(1280.0), (canvas.height() - 40.0).min(760.0)),
        );
        if let Some(size) = self.assistant.summon.sheet_request.take() {
            self.assistant.summon.sheet = (size[0] > 0.0).then(|| {
                Rect::from_center_size(
                    pos2(canvas.center().x, canvas.bottom() - 24.0 - size[1] / 2.0),
                    vec2(size[0], size[1]),
                )
            });
        }
        let mut rect = self.assistant.summon.sheet.unwrap_or(default);
        rect = fit(rect, canvas);
        rect
    }

    // ---- 1. split ---------------------------------------------------------------------

    fn layout_split(
        &mut self,
        ui: &mut Ui,
        asks: &[Ask],
        action: &mut Option<Action>,
        allow_low: &mut bool,
        answers: &mut Vec<(PanelId, bool)>,
    ) {
        self.dock_header(ui, action);
        ui.add_space(8.0);
        self.scope_strip(ui);
        ui.add_space(8.0);
        let area = ui.available_rect_before_wrap();
        let ratio = if self.assistant.summon.split <= 0.0 {
            0.5
        } else {
            self.assistant.summon.split.clamp(0.28, 0.72)
        };
        let divider_x = area.left() + area.width() * ratio;
        let left = Rect::from_min_max(area.min, pos2(divider_x - 6.0, area.bottom()));
        let right = Rect::from_min_max(pos2(divider_x + 6.0, area.top()), area.max);
        ui.allocate_rect(area, Sense::hover());
        // The divider: drag to give one side more room.
        let handle = Rect::from_min_max(pos2(divider_x - 6.0, area.top()), pos2(divider_x + 6.0, area.bottom()));
        let response = ui.interact(handle, Id::new("split_divider"), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if response.dragged() {
            self.assistant.summon.split =
                ((divider_x + response.drag_delta().x - area.left()) / area.width()).clamp(0.28, 0.72);
        }
        ui.painter().rect_filled(
            Rect::from_center_size(handle.center(), vec2(3.0, 56.0)),
            CornerRadius::same(2),
            if response.hovered() {
                theme::ACCENT()
            } else {
                theme::BORDER_STRONG()
            },
        );
        let mut voice = ui.new_child(egui::UiBuilder::new().max_rect(left));
        self.voice_pane(&mut voice, asks, action, allow_low, answers);
        let mut text = ui.new_child(egui::UiBuilder::new().max_rect(right));
        self.text_pane(&mut text, right.height());
    }

    /// The voice assistant: questions gathered, the conversation, the mic.
    fn voice_pane(
        &mut self,
        ui: &mut Ui,
        asks: &[Ask],
        action: &mut Option<Action>,
        allow_low: &mut bool,
        answers: &mut Vec<(PanelId, bool)>,
    ) {
        pane_title(ui, "Voice assistant", "Talks to you, and runs Horizon for you");
        ui.add_space(8.0);
        if !asks.is_empty() {
            approvals_card(ui, asks, allow_low, answers);
            ui.add_space(8.0);
        }
        let body = (ui.available_height() - 62.0 - 8.0).max(120.0);
        self.conversation(ui, body, false);
        ui.add_space(8.0);
        self.desk_prompt(ui, action);
    }

    /// The text agent: its engine, its scope and its real terminal.
    fn text_pane(&mut self, ui: &mut Ui, height: f32) {
        pane_title(ui, "Text agent", "A real agent. Type to it.");
        ui.add_space(8.0);
        self.engine_tabs(ui);
        ui.add_space(8.0);
        let remaining = (height - 96.0).max(120.0);
        self.agent_terminal(ui, remaining, None);
    }

    // ---- 2. feed ----------------------------------------------------------------------

    fn layout_feed(
        &mut self,
        ui: &mut Ui,
        asks: &[Ask],
        action: &mut Option<Action>,
        allow_low: &mut bool,
        answers: &mut Vec<(PanelId, bool)>,
    ) {
        self.dock_header(ui, action);
        ui.add_space(8.0);
        self.scope_strip(ui);
        ui.add_space(8.0);
        let in_room =
            self.assistant.summon.msgs.on && matches!(self.assistant.summon.msgs.view, super::messages::View::Room(_));
        if in_room {
            let height = ui.available_height();
            self.message_room(ui, height);
            return;
        }
        if self.assistant.summon.msgs.on {
            self.message_inbox(ui);
            ui.add_space(8.0);
        }
        if !asks.is_empty() {
            approvals_card(ui, asks, allow_low, answers);
            ui.add_space(8.0);
        }
        let drawer_height = if self.assistant.summon.drawer <= 0.0 {
            260.0
        } else {
            self.assistant.summon.drawer
        };
        let drawer = if self.assistant.summon.drawer_open {
            drawer_height.clamp(140.0, 520.0)
        } else {
            0.0
        };
        // What sits under the conversation: the channel row, the prompt and, when open, the drawer.
        let below = 30.0
            + 4.0
            + 62.0
            + if drawer > 0.0 {
                8.0 + 14.0 + 34.0 + 6.0 + drawer
            } else {
                0.0
            };
        let body = (ui.available_height() - below - 8.0).max(90.0);
        self.conversation(ui, body, false);
        ui.add_space(6.0);
        self.channel_row(ui);
        ui.add_space(4.0);
        self.desk_prompt(ui, action);
        if self.assistant.summon.drawer_open {
            ui.add_space(8.0);
            // The grip: drag up for a taller drawer.
            let (grip, response) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            }
            if response.dragged() {
                self.assistant.summon.drawer = (drawer_height - response.drag_delta().y).clamp(140.0, 520.0);
            }
            ui.painter().rect_filled(
                Rect::from_center_size(grip.center(), vec2(54.0, 4.0)),
                CornerRadius::same(2),
                theme::BORDER_STRONG(),
            );
            self.engine_tabs(ui);
            ui.add_space(6.0);
            self.agent_terminal(ui, drawer, None);
        }
    }

    /// Which way the composer sends: to the voice assistant, or as text to the agent.
    fn channel_row(&mut self, ui: &mut Ui) {
        let agent = horizon_core::agent_definition(self.assistant.settings.agent).map_or("Agent", |a| a.display_name);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let channel = self.assistant.summon.channel;
            if mode_chip(ui, "Voice assistant", channel == Channel::Voice).clicked() {
                self.assistant.summon.channel = Channel::Voice;
            }
            if mode_chip(ui, &format!("Text to {agent}"), channel == Channel::Text).clicked() {
                self.assistant.summon.channel = Channel::Text;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let open = self.assistant.summon.drawer_open;
                if mode_chip(ui, if open { "Hide terminal" } else { "Terminal" }, open).clicked() {
                    self.assistant.summon.drawer_open = !open;
                }
            });
        });
    }

    // ---- 3. roster --------------------------------------------------------------------

    fn layout_roster(
        &mut self,
        ui: &mut Ui,
        asks: &[Ask],
        action: &mut Option<Action>,
        allow_low: &mut bool,
        answers: &mut Vec<(PanelId, bool)>,
    ) {
        self.dock_header(ui, action);
        ui.add_space(8.0);
        self.scope_strip(ui);
        ui.add_space(8.0);
        match self.assistant.summon.pick {
            Pick::Voice => {
                if !asks.is_empty() {
                    approvals_card(ui, asks, allow_low, answers);
                    ui.add_space(8.0);
                }
                let body = (ui.available_height() - 62.0 - 8.0).max(120.0);
                self.conversation(ui, body, false);
                ui.add_space(8.0);
                self.desk_prompt(ui, action);
            }
            Pick::Text => {
                self.engine_tabs(ui);
                ui.add_space(8.0);
                let h = (ui.available_height() - 4.0).max(120.0);
                self.agent_terminal(ui, h, None);
            }
            Pick::Worker(id) => {
                let title = self
                    .board
                    .panel(id)
                    .map_or_else(String::new, |panel| panel.display_title().into_owned());
                pane_title(ui, &title, "A worker in scope. Its own terminal.");
                ui.add_space(8.0);
                let h = (ui.available_height() - 4.0).max(120.0);
                self.agent_terminal(ui, h, Some(id));
            }
        }
    }

    /// The roster: who is here, and what they are doing.
    fn roster_rail(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("WHO IS HERE")
                .size(10.5)
                .extra_letter_spacing(0.9)
                .color(theme::FG_DIM()),
        );
        ui.add_space(8.0);
        let engine = horizon_core::agent_definition(self.assistant.settings.agent).map_or("Agent", |a| a.display_name);
        let voice_busy = self.speaking_now();
        let text_state = self.board.assistant_panel().and_then(|id| self.board.agent_state(id));
        let pick = self.assistant.summon.pick;
        if roster_row(
            ui,
            "Voice assistant",
            "Talks, and runs Horizon",
            dot(voice_busy.then_some(AgentState::Working)),
            pick == Pick::Voice,
        )
        .clicked()
        {
            self.assistant.summon.pick = Pick::Voice;
        }
        if roster_row(
            ui,
            &format!("{engine}  (text)"),
            "Type to a real agent",
            dot(text_state),
            pick == Pick::Text,
        )
        .clicked()
        {
            self.assistant.summon.pick = Pick::Text;
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new("WORKERS IN SCOPE")
                .size(10.5)
                .extra_letter_spacing(0.9)
                .color(theme::FG_DIM()),
        );
        ui.add_space(6.0);
        let workers = self.feed_agents();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 110.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for worker in &workers {
                    let selected = pick == Pick::Worker(worker.id);
                    if roster_row(
                        ui,
                        &worker.title,
                        &elide(&worker.workspace_name, 22),
                        dot(Some(worker.state)),
                        selected,
                    )
                    .clicked()
                    {
                        self.assistant.summon.pick = Pick::Worker(worker.id);
                    }
                }
                if workers.is_empty() {
                    ui.label(RichText::new("None in this scope.").size(12.0).color(theme::FG_DIM()));
                }
            });
    }

    // ---- shared -----------------------------------------------------------------------

    /// Claude, Codex or Grok: the engine of the text agent. A press restarts it with that engine.
    pub(super) fn engine_tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for kind in ASSISTANT_AGENTS.into_iter().filter(|kind| {
                matches!(
                    kind,
                    horizon_core::PanelKind::Claude | horizon_core::PanelKind::Codex | horizon_core::PanelKind::Grok
                )
            }) {
                let name = horizon_core::agent_definition(kind).map_or("Agent", |a| a.display_name);
                let selected = self.assistant.settings.agent == kind;
                if mode_chip(ui, name, selected).clicked() && !selected {
                    self.assistant.draft.agent = kind;
                    self.apply_assistant_engine();
                }
            }
            ui.add_space(6.0);
            static_chip(ui, &format!("May do  {}", self.assistant.settings.mode.label()));
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("Looking at  {}", self.scope_label()))
                    .size(11.5)
                    .color(theme::FG_DIM()),
            );
        });
    }

    /// An agent's real terminal, in a rounded frame, `height` tall. `None` is the text agent.
    pub(super) fn agent_terminal(&mut self, ui: &mut Ui, height: f32, panel: Option<PanelId>) {
        let Some(id) = panel.or_else(|| self.board.assistant_panel()) else {
            ui.label(RichText::new("Starting the agent...").color(theme::FG_DIM()));
            return;
        };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(12), theme::BG());
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let mut body = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 10.0))));
        if self.show_assistant_terminal(&mut body, id) {
            self.focus_assistant();
        }
    }
}

// ---- chrome and painting -------------------------------------------------------------------

/// The window kept inside the canvas and above a usable size.
fn fit(rect: Rect, canvas: Rect) -> Rect {
    let width = rect
        .width()
        .clamp(MIN_SIZE[0].min(canvas.width() - 16.0), canvas.width() - 16.0);
    let height = rect
        .height()
        .clamp(MIN_SIZE[1].min(canvas.height() - 16.0), canvas.height() - 16.0);
    let left = rect.left().clamp(
        canvas.left() + 8.0,
        (canvas.right() - width - 8.0).max(canvas.left() + 8.0),
    );
    let top = rect.top().clamp(
        canvas.top() + 8.0,
        (canvas.bottom() - height - 8.0).max(canvas.top() + 8.0),
    );
    Rect::from_min_size(pos2(left, top), vec2(width, height))
}

/// Edge and corner handles; returns the rectangle after this frame's drags.
fn resize(ui: &mut Ui, rect: Rect, current: Rect) -> Rect {
    let mut next = current;
    let zones = [
        (
            "l",
            Rect::from_min_max(
                pos2(rect.left() - EDGE, rect.top() + 14.0),
                pos2(rect.left() + EDGE, rect.bottom() - 14.0),
            ),
            CursorIcon::ResizeHorizontal,
            [1.0, 0.0, 0.0, 0.0],
        ),
        (
            "r",
            Rect::from_min_max(
                pos2(rect.right() - EDGE, rect.top() + 14.0),
                pos2(rect.right() + EDGE, rect.bottom() - 14.0),
            ),
            CursorIcon::ResizeHorizontal,
            [0.0, 0.0, 1.0, 0.0],
        ),
        (
            "t",
            Rect::from_min_max(
                pos2(rect.left() + 14.0, rect.top() - EDGE),
                pos2(rect.right() - 14.0, rect.top() + EDGE),
            ),
            CursorIcon::ResizeVertical,
            [0.0, 1.0, 0.0, 0.0],
        ),
        (
            "b",
            Rect::from_min_max(
                pos2(rect.left() + 14.0, rect.bottom() - EDGE),
                pos2(rect.right() - 14.0, rect.bottom() + EDGE),
            ),
            CursorIcon::ResizeVertical,
            [0.0, 0.0, 0.0, 1.0],
        ),
        (
            "tl",
            Rect::from_center_size(rect.left_top(), vec2(2.0 * 14.0, 2.0 * 14.0)),
            CursorIcon::ResizeNwSe,
            [1.0, 1.0, 0.0, 0.0],
        ),
        (
            "tr",
            Rect::from_center_size(rect.right_top(), vec2(2.0 * 14.0, 2.0 * 14.0)),
            CursorIcon::ResizeNeSw,
            [0.0, 1.0, 1.0, 0.0],
        ),
        (
            "bl",
            Rect::from_center_size(rect.left_bottom(), vec2(2.0 * 14.0, 2.0 * 14.0)),
            CursorIcon::ResizeNeSw,
            [1.0, 0.0, 0.0, 1.0],
        ),
        (
            "br",
            Rect::from_center_size(rect.right_bottom(), vec2(2.0 * 14.0, 2.0 * 14.0)),
            CursorIcon::ResizeNwSe,
            [0.0, 0.0, 1.0, 1.0],
        ),
    ];
    for (name, zone, cursor, weights) in zones {
        let response = ui.interact(zone, Id::new(("sheet_resize", name)), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.dragged() {
            let delta = response.drag_delta();
            next = Rect::from_min_max(
                pos2(next.left() + weights[0] * delta.x, next.top() + weights[1] * delta.y),
                pos2(
                    next.right() + weights[2] * delta.x,
                    next.bottom() + weights[3] * delta.y,
                ),
            );
        }
    }
    next
}

fn pane_title(ui: &mut Ui, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(14.0).strong().color(theme::FG()));
        ui.label(RichText::new(detail).size(11.5).color(theme::FG_DIM()));
    });
}

fn dot(state: Option<AgentState>) -> Color32 {
    match state {
        Some(AgentState::Working) => theme::PALETTE_YELLOW(),
        Some(AgentState::NeedsInput) => theme::PALETTE_RED(),
        Some(AgentState::Idle) => theme::PALETTE_GREEN(),
        _ => theme::BORDER_STRONG(),
    }
}

fn roster_row(ui: &mut Ui, title: &str, detail: &str, color: Color32, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(10),
            if selected {
                theme::ACCENT().gamma_multiply(0.16)
            } else {
                theme::PANEL_BG_ALT()
            },
        );
    }
    ui.painter()
        .circle_filled(rect.left_center() + vec2(14.0, 0.0), 4.0, color);
    ui.painter().text(
        rect.left_top() + vec2(30.0, 16.0),
        Align2::LEFT_CENTER,
        elide(title, 24),
        FontId::proportional(13.0),
        if selected { theme::FG() } else { theme::FG_SOFT() },
    );
    ui.painter().text(
        rect.left_top() + vec2(30.0, 34.0),
        Align2::LEFT_CENTER,
        elide(detail, 28),
        FontId::proportional(11.0),
        theme::FG_DIM(),
    );
    response
}
