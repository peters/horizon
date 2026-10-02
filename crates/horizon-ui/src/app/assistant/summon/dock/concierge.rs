//! Design 1, the concierge: one conversation, and the assistant stands between you and your agents.
//!
//! Mini: a pill. When agents ask things it shows the single most careful question, with Allow and Deny
//! in the pill itself, a count of the others, and one button for the harmless ones. Nothing stacks.
//! Expanded: a conversation with a rail of threads on the left, everywhere and per workspace, and the
//! questions gathered into one card at the top instead of scattered through the chat.

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Rect, RichText,
    ScrollArea, Sense, Shadow, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::super::super::{icons, num};
use super::super::desk_bar::paint::elide;
use super::super::mini::{MiniAction, chevron_button, round_button, small_button};
use super::super::{Action, HorizonApp, INPUT_ID, demo};
use super::inbox::{Ask, Risk};
use crate::theme;

const PILL_HEIGHT: f32 = 64.0;
const PILL_WIDTH: f32 = 800.0;
const MARGIN_BOTTOM: f32 = 24.0;
/// The global space of threads: not attached to a workspace.
const EVERYWHERE: &str = "Everywhere";

impl HorizonApp {
    // ---- mini --------------------------------------------------------------------------

    pub(super) fn concierge_mini(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        let width = PILL_WIDTH.min(canvas.width() - 48.0).max(420.0);
        let asks = self.inbox();
        let mut actions: Vec<MiniAction> = Vec::new();
        let mut key = None;
        let mut allow_low = false;
        let id = Id::new(INPUT_ID);
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(canvas.center().x, canvas.bottom() - MARGIN_BOTTOM) + self.assistant.summon.mini_offset)
            .show(ctx, |ui| {
                let (pill, _) = ui.allocate_exact_size(vec2(width, PILL_HEIGHT), Sense::hover());
                (key, allow_low) = self.paint_concierge_pill(ui, pill, id, &asks, &mut actions);
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
        if allow_low {
            self.allow_low_risk();
        }
        match key {
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            Some(Action::OpenAsChat | Action::Complete(_)) => self.assistant.summon.expanded = true,
            _ => {}
        }
    }

    fn paint_concierge_pill(
        &mut self,
        ui: &mut Ui,
        pill: Rect,
        id: Id,
        asks: &[Ask],
        actions: &mut Vec<MiniAction>,
    ) -> (Option<Action>, bool) {
        let radius = CornerRadius::same(32);
        let agents = self.feed_agents();
        let now = num::seconds(ui);
        let listening = self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening);
        let speaking = self.speaking_now();
        let accent = match asks.first() {
            _ if self.assistant.summon.ckpt.has() => theme::PALETTE_RED(),
            Some(ask) => ask.risk.color(),
            None if listening || speaking => theme::ACCENT(),
            None => theme::ACCENT().gamma_multiply(0.55),
        };
        ui.painter().add(
            Shadow {
                offset: [0, 10],
                blur: 34,
                spread: 0,
                color: Color32::from_black_alpha(130),
            }
            .as_shape(pill, radius),
        );
        ui.painter().rect_filled(pill, radius, theme::BG_ELEVATED());
        ui.painter()
            .rect_stroke(pill, radius, Stroke::new(1.3, accent), StrokeKind::Inside);
        let centre = pos2(pill.left() + 38.0, pill.center().y);
        self.paint_orb(ui, centre, 24.0, &agents, now);
        let orb = ui
            .interact(
                Rect::from_center_size(centre, vec2(52.0, 52.0)),
                Id::new("concierge_orb"),
                Sense::click_and_drag(),
            )
            .on_hover_text("Click to open, drag to move");
        if orb.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            actions.push(MiniAction::Drag(orb.drag_delta()));
        } else if orb.clicked() {
            actions.push(MiniAction::Expand);
        }
        let expand = Rect::from_center_size(pos2(pill.right() - 32.0, pill.center().y), vec2(34.0, 34.0));
        let mic = Rect::from_center_size(pos2(pill.right() - 76.0, pill.center().y), vec2(38.0, 38.0));
        if chevron_button(ui, expand).clicked() {
            actions.push(MiniAction::Expand);
        }
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
        let left = pill.left() + 76.0;
        let right = mic.left() - 12.0;
        if self.assistant.summon.ckpt.has() {
            // A checkpoint comes first: it is what the agents agreed to stop for.
            let press = self.paint_top_checkpoint(ui, pill, (left, right));
            self.apply_checkpoint_press(press);
            return (None, false);
        }
        let Some(top) = asks.first() else {
            if self.paint_newest_message(ui, pill, (left, right), actions) {
                return (None, false);
            }
            let field = Rect::from_min_max(pos2(left, pill.top() + 8.0), pos2(right, pill.bottom() - 8.0));
            let hint = self.concierge_hint(listening);
            let key = self.paint_pill_field(
                ui,
                field,
                id,
                &hint,
                listening || speaking,
                self.assistant_busy() || speaking,
            );
            return (key, false);
        };
        let allow_low = self.paint_top_ask(ui, pill, (left, right), asks, top, actions);
        (None, allow_low)
    }

    /// The newest unread message in the pill: who, where, what, and a way in. Returns whether there was one.
    fn paint_newest_message(
        &mut self,
        ui: &mut Ui,
        pill: Rect,
        (left, right): (f32, f32),
        actions: &mut Vec<MiniAction>,
    ) -> bool {
        if !self.assistant.summon.msgs.on {
            return false;
        }
        let Some((from, tag, text)) = self.assistant.summon.msgs.newest_unread() else {
            return false;
        };
        ui.painter().text(
            pos2(left, pill.center().y - 9.0),
            Align2::LEFT_CENTER,
            format!("{from}  {tag}"),
            FontId::proportional(11.5),
            theme::PALETTE_CYAN(),
        );
        ui.painter().text(
            pos2(left, pill.center().y + 9.0),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(13.5),
            theme::FG(),
        );
        let open = Rect::from_center_size(pos2(right - 40.0, pill.center().y), vec2(76.0, 30.0));
        if small_button(ui, open, "Open", true).clicked() {
            self.assistant.summon.msgs.view = super::messages::View::Concierge;
            actions.push(MiniAction::Expand);
        }
        ui.ctx().request_repaint();
        true
    }

    /// The one question that matters most, answered in place. Returns whether "allow the low-risk ones" was pressed.
    fn paint_top_ask(
        &self,
        ui: &mut Ui,
        pill: Rect,
        (left, right): (f32, f32),
        asks: &[Ask],
        top: &Ask,
        actions: &mut Vec<MiniAction>,
    ) -> bool {
        // The one question that matters most, answered in place.
        let mut x = left;
        x = risk_chip(ui, pos2(x, pill.center().y), top.risk) + 10.0;
        let allow = Rect::from_center_size(pos2(right - 138.0, pill.center().y), vec2(64.0, 30.0));
        let deny = Rect::from_center_size(pos2(right - 66.0, pill.center().y), vec2(62.0, 30.0));
        let low = asks.iter().filter(|ask| ask.risk == Risk::Low).count();
        let more = asks.len() - 1;
        let tail = if low >= 2 && more > 0 {
            168.0
        } else if more > 0 {
            56.0
        } else {
            0.0
        };
        let text_room = (allow.left() - tail - x - 8.0).max(60.0);
        ui.painter().text(
            pos2(x, pill.center().y - 9.0),
            Align2::LEFT_CENTER,
            elide(
                &format!("{} in {}", top.agent, top.workspace),
                num::index(text_room / 6.6),
            ),
            FontId::proportional(11.5),
            theme::FG_DIM(),
        );
        ui.painter().text(
            pos2(x, pill.center().y + 9.0),
            Align2::LEFT_CENTER,
            elide(&top.text, num::index(text_room / 7.0)),
            FontId::proportional(13.5),
            theme::FG(),
        );
        if small_button(ui, allow, "Allow", true).clicked() {
            actions.push(MiniAction::Answer(top.id, true));
        }
        if let Some(progress) = self.assistant.demo.as_ref().and_then(demo::Demo::press_progress) {
            super::super::desk_bar::paint::paint_click(ui, allow.center(), progress);
        }
        if small_button(ui, deny, "Deny", false).clicked() {
            actions.push(MiniAction::Answer(top.id, false));
        }
        let mut allow_low = false;
        let mut tail_x = allow.left() - 10.0;
        if low >= 2 && more > 0 {
            let button = Rect::from_min_max(
                pos2(tail_x - 118.0, pill.center().y - 14.0),
                pos2(tail_x, pill.center().y + 14.0),
            );
            allow_low = small_button(ui, button, &format!("Allow {low} low-risk"), false).clicked();
            tail_x = button.left() - 8.0;
        }
        if more > 0 {
            count_chip(ui, pos2(tail_x, pill.center().y), &format!("+{more}"));
        }
        ui.ctx().request_repaint();
        allow_low
    }

    fn concierge_hint(&self, listening: bool) -> String {
        if listening {
            return "Listening...".to_string();
        }
        if let Some(line) = self.assistant.demo.as_ref().and_then(demo::Demo::speaking_line) {
            return elide(line, 90);
        }
        if self.assistant_busy() {
            return self
                .assistant
                .feed
                .latest_did()
                .map_or_else(|| "Working on it...".to_string(), |step| elide(step, 90));
        }
        "Ask anything, or tell me what to do".to_string()
    }

    // ---- expanded ----------------------------------------------------------------------

    /// Threads: global ones first, then the ones of the workspace in scope.
    pub(super) fn thread_rail(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("THREADS")
                .size(10.5)
                .extra_letter_spacing(0.9)
                .color(theme::FG_DIM()),
        );
        ui.add_space(8.0);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        ui.painter().rect(
            rect,
            CornerRadius::same(10),
            if response.hovered() {
                theme::ACCENT().gamma_multiply(0.3)
            } else {
                theme::ACCENT().gamma_multiply(0.2)
            },
            Stroke::new(1.0, theme::ACCENT().gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "+  New thread",
            FontId::proportional(13.0),
            theme::FG(),
        );
        ui.add_space(10.0);
        let scope = self.scope_label();
        let scoped = !self.assistant.scope.is_all();
        let groups: Vec<(String, Vec<(String, String)>)> = self
            .assistant
            .threads
            .by_space()
            .into_iter()
            .map(|(space, threads)| {
                (
                    space.to_string(),
                    threads
                        .into_iter()
                        .map(|thread| (thread.title.clone(), age(thread.updated_at)))
                        .collect(),
                )
            })
            .collect();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let shown: Vec<&(String, Vec<(String, String)>)> = groups
                .iter()
                .filter(|(space, _)| space == EVERYWHERE || (scoped && *space == scope))
                .collect();
            for (space, threads) in shown {
                ui.label(
                    RichText::new(if space == EVERYWHERE {
                        "EVERYWHERE"
                    } else {
                        "THIS WORKSPACE"
                    })
                    .size(10.0)
                    .extra_letter_spacing(0.8)
                    .color(theme::FG_DIM()),
                );
                if space != EVERYWHERE {
                    ui.label(RichText::new(space.clone()).size(11.5).color(theme::ACCENT()));
                }
                ui.add_space(4.0);
                for (index, (title, when)) in threads.iter().enumerate() {
                    thread_row(ui, title, when, space == EVERYWHERE && index == 0);
                }
                ui.add_space(10.0);
            }
            if groups.is_empty() {
                ui.label(RichText::new("No threads yet.").size(12.0).color(theme::FG_DIM()));
            }
        });
    }
}

// ---- painting ---------------------------------------------------------------------------

/// A question with its risk and Allow and Deny, and above the rows the one button for the harmless ones.
pub(super) fn approvals_card(
    ui: &mut Ui,
    asks: &[Ask],
    allow_low: &mut bool,
    answers: &mut Vec<(horizon_core::PanelId, bool)>,
) {
    let low = asks.iter().filter(|ask| ask.risk == Risk::Low).count();
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG(), theme::PALETTE_YELLOW(), 0.07))
        .stroke(Stroke::new(1.2, theme::PALETTE_YELLOW().gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} {} you",
                        asks.len(),
                        if asks.len() == 1 { "needs" } else { "need" }
                    ))
                    .size(14.0)
                    .strong()
                    .color(theme::FG()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if low >= 2 && super::super::super::blocks::primary(ui, &format!("Allow {low} low-risk")).clicked()
                    {
                        *allow_low = true;
                    }
                });
            });
            ui.add_space(6.0);
            ScrollArea::vertical()
                .max_height(176.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for ask in asks {
                        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
                        let mid = row.center().y;
                        let after = risk_chip(ui, pos2(row.left(), mid), ask.risk) + 10.0;
                        ui.painter().text(
                            pos2(after, mid - 9.0),
                            Align2::LEFT_CENTER,
                            elide(&format!("{} in {}", ask.agent, ask.workspace), 40),
                            FontId::proportional(11.0),
                            theme::FG_DIM(),
                        );
                        ui.painter().text(
                            pos2(after, mid + 8.0),
                            Align2::LEFT_CENTER,
                            elide(&ask.text, num::index((row.right() - after - 160.0) / 7.0)),
                            FontId::proportional(13.0),
                            theme::FG(),
                        );
                        let allow = Rect::from_center_size(pos2(row.right() - 100.0, mid), vec2(62.0, 28.0));
                        let deny = Rect::from_center_size(pos2(row.right() - 32.0, mid), vec2(58.0, 28.0));
                        if row_button(ui, allow, "Allow", true, ask.id.0).clicked() {
                            answers.push((ask.id, true));
                        }
                        if row_button(ui, deny, "Deny", false, ask.id.0).clicked() {
                            answers.push((ask.id, false));
                        }
                    }
                });
        });
}

fn row_button(ui: &mut Ui, rect: Rect, text: &str, primary: bool, key: u64) -> egui::Response {
    let response = ui.interact(rect, Id::new(("ask_button", text, key)), Sense::click());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(14),
        if primary {
            theme::ACCENT()
        } else if response.hovered() {
            theme::BORDER_SUBTLE()
        } else {
            theme::PANEL_BG_ALT()
        },
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.5),
        if primary {
            Color32::from_rgb(7, 16, 31)
        } else {
            theme::FG_SOFT()
        },
    );
    response
}

/// A risk pill; returns the right edge.
pub(super) fn risk_chip(ui: &Ui, left_centre: egui::Pos2, risk: Risk) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(risk.label().to_string(), FontId::proportional(11.0), risk.color());
    let rect = Rect::from_min_size(
        pos2(left_centre.x, left_centre.y - 11.0),
        vec2(galley.size().x + 22.0, 22.0),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        risk.color().gamma_multiply(0.16),
        Stroke::new(1.0, risk.color().gamma_multiply(0.5)),
        StrokeKind::Inside,
    );
    ui.painter()
        .circle_filled(rect.left_center() + vec2(10.0, 0.0), 3.0, risk.color());
    ui.painter().galley(
        rect.left_center() + vec2(18.0, -galley.size().y / 2.0),
        galley,
        risk.color(),
    );
    rect.right()
}

/// A small neutral count, to the left of `right_centre`.
pub(super) fn count_chip(ui: &Ui, right_centre: egui::Pos2, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(12.0), theme::FG_SOFT());
    let size = vec2(galley.size().x + 16.0, 24.0);
    let rect = Rect::from_min_size(pos2(right_centre.x - size.x, right_centre.y - 12.0), size);
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.left_center() + vec2(8.0, -galley.size().y / 2.0),
        galley,
        theme::FG_SOFT(),
    );
}

pub(super) fn mode_chip(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let button =
        egui::Button::new(
            RichText::new(text)
                .size(12.0)
                .color(if selected { theme::FG() } else { theme::FG_DIM() }),
        )
        .fill(if selected {
            theme::ACCENT().gamma_multiply(0.26)
        } else {
            theme::PANEL_BG()
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::ACCENT()
            } else {
                theme::BORDER_SUBTLE()
            },
        ))
        .corner_radius(CornerRadius::same(99))
        .min_size(vec2(0.0, 26.0));
    ui.add(button)
}

fn thread_row(ui: &mut Ui, title: &str, when: &str, active: bool) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(10),
            if active {
                theme::ACCENT().gamma_multiply(0.14)
            } else {
                theme::PANEL_BG_ALT()
            },
        );
    }
    ui.painter().text(
        rect.left_top() + vec2(10.0, 15.0),
        Align2::LEFT_CENTER,
        elide(title, 26),
        FontId::proportional(12.5),
        if active { theme::FG() } else { theme::FG_SOFT() },
    );
    ui.painter().text(
        rect.left_top() + vec2(10.0, 32.0),
        Align2::LEFT_CENTER,
        when,
        FontId::proportional(10.5),
        theme::FG_DIM(),
    );
}

/// How long ago a thread was used, from its unix milliseconds.
fn age(updated_ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0));
    let minutes = ((now - updated_ms) / 60_000).max(0);
    match minutes {
        0..=1 => "just now".to_string(),
        2..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} d ago", minutes / 1440),
    }
}
