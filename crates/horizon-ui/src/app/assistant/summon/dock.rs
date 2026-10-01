//! The assistant inside Horizon's own window.
//!
//! A slim pill rests at the bottom of the canvas: the orb (its ring has an arc for each agent), a
//! field to type into or dictate to, and a mic. News arrives as cards stacked above it: work under
//! way, a question with Allow and Deny, a finished result. One click opens the whole conversation as
//! a sheet, as cards or chat, with the raw terminal one click away. Everything is drawn in
//! the window the person is already in, so nothing needs the desktop's help.
//!
//! Switched on with `HORIZON_ASSISTANT_DOCK=1` while it is a prototype.

use std::sync::OnceLock;

use egui::{
    Align, Align2, Area, Color32, Context, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Rect, RichText,
    Sense, Shadow, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2,
};

use super::super::{command_bar, icons, num};
use super::deck::{CARD_HEIGHT, GAP};
use super::desk_bar::{Tile, paint};
use super::mini::{MiniAction, chevron_button, round_button};
use super::turns::FeedStyle;
use super::{Action, HorizonApp, INPUT_ID, demo, waveform};
use crate::app::assistant::command_bar::LocalCommand;
use crate::theme;

const PILL_HEIGHT: f32 = 64.0;
const PILL_WIDTH: f32 = 600.0;
const SHEET_WIDTH: f32 = 800.0;
const SHEET_HEIGHT: f32 = 720.0;
const MARGIN_BOTTOM: f32 = 24.0;

/// Whether the dock replaces the old summon bar and drawer.
pub(in crate::app) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HORIZON_ASSISTANT_DOCK").is_some_and(|value| !value.is_empty()))
}

impl HorizonApp {
    /// The dock for this frame: the pill with its cards, or the sheet.
    pub(in crate::app) fn render_assistant_dock(&mut self, ctx: &Context) {
        if !enabled() || self.settings.is_some() || self.fullscreen_panel.is_some() {
            return;
        }
        self.run_demo(ctx);
        self.ensure_assistant_panel(ctx);
        self.follow_assistant_transcript();
        if let Some(index) = self.assistant.summon.pending_workspace.take()
            && let Some(id) = self.board.workspaces.get(index).map(|workspace| workspace.id)
        {
            self.focus_workspace_visible(ctx, id, false);
        }
        if self.assistant.summon.expanded {
            self.dock_sheet(ctx);
        } else {
            self.dock_pill(ctx);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    fn dock_anchor(&self, ctx: &Context) -> (Rect, egui::Pos2) {
        let canvas = self.canvas_rect(ctx);
        (canvas, pos2(canvas.center().x, canvas.bottom() - MARGIN_BOTTOM))
    }

    // ---- the pill ----------------------------------------------------------------------

    fn dock_pill(&mut self, ctx: &Context) {
        let (canvas, anchor) = self.dock_anchor(ctx);
        let toasts = self.deck_toasts();
        let stack = super::super::num::count(toasts.len()) * (CARD_HEIGHT + GAP);
        let width = PILL_WIDTH.min(canvas.width() - 48.0).max(360.0);
        let mut actions: Vec<MiniAction> = Vec::new();
        let mut submit = None;
        let id = Id::new(INPUT_ID);
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                let (whole, _) = ui.allocate_exact_size(vec2(width, stack + PILL_HEIGHT), Sense::hover());
                if !toasts.is_empty() {
                    let cards = Rect::from_min_size(whole.min, vec2(width, stack));
                    self.paint_toasts(ui, cards, &toasts, &mut actions);
                }
                let pill = Rect::from_min_size(
                    pos2(whole.left(), whole.bottom() - PILL_HEIGHT),
                    vec2(width, PILL_HEIGHT),
                );
                submit = self.paint_pill(ui, pill, id, &mut actions);
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        if ctx.memory(|memory| memory.has_focus(id)) && self.board.focused.is_some() {
            // A click behind the field must not leave a panel typing alongside it.
            self.claim_keyboard_for_bar();
        }
        for action in actions {
            self.apply_dock_action(ctx, &action);
        }
        match submit {
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            Some(Action::OpenAsChat | Action::Complete(_)) => self.assistant.summon.expanded = true,
            _ => {}
        }
        // A command being typed needs the room the sheet has for its suggestions.
        if self.assistant.summon.text.starts_with('/') {
            self.assistant.summon.expanded = true;
            self.assistant.summon.focus_requested = true;
        }
    }

    fn apply_dock_action(&mut self, ctx: &Context, action: &MiniAction) {
        match *action {
            MiniAction::Expand => {
                self.assistant.summon.expanded = true;
                self.assistant.summon.focus_requested = true;
            }
            MiniAction::Open { terminal } => {
                self.assistant.summon.expanded = true;
                self.assistant.summon.feed_style = FeedStyle::Cards;
                self.assistant.summon.raw = terminal;
            }
            MiniAction::Dismiss(turn) => self.assistant.summon.deck_dismissed = turn + 1,
            MiniAction::Dictate => self.toggle_assistant_dictation(ctx),
            MiniAction::Answer(id, allow) => self.answer_agent(id, allow),
            MiniAction::Go(_) | MiniAction::Style(_) => {}
        }
    }

    /// The capsule. Returns what a key press in its field asked for.
    fn paint_pill(&mut self, ui: &mut Ui, pill: Rect, id: Id, actions: &mut Vec<MiniAction>) -> Option<Action> {
        let agents = self.feed_agents();
        let now = num::seconds(ui);
        let radius = CornerRadius::same(32);
        let listening = self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening);
        let speaking = self.speaking_now();
        let working = self.assistant_busy();
        let accent = if listening || speaking {
            theme::ACCENT()
        } else if agents
            .iter()
            .any(|agent| agent.state == horizon_core::browser::manifest::agent_panels::AgentState::NeedsInput)
        {
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
            .as_shape(pill, radius),
        );
        ui.painter().rect_filled(pill, radius, theme::BG_ELEVATED());
        ui.painter()
            .rect_stroke(pill, radius, Stroke::new(1.2, accent), StrokeKind::Inside);

        // The orb.
        let centre = pos2(pill.left() + 38.0, pill.center().y);
        self.paint_orb(ui, centre, 24.0, &agents, now);
        let orb_hit = Rect::from_center_size(centre, vec2(52.0, 52.0));
        if ui
            .interact(orb_hit, Id::new("dock_orb"), Sense::click())
            .on_hover_text("Open the conversation")
            .clicked()
        {
            actions.push(MiniAction::Expand);
        }

        // The buttons on the right, and the shortcut that gets here from anywhere.
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
        let shortcut = self
            .shortcuts
            .summon_assistant
            .display_label(crate::app::util::primary_shortcut_label());
        let chip_right = mic.left() - 10.0;
        let chip_width = if self.assistant.summon.text.is_empty() && !listening {
            paint_key_chip(ui, pos2(chip_right, pill.center().y), &shortcut)
        } else {
            0.0
        };

        // The field: type, or watch the words arrive while someone speaks.
        let field = Rect::from_min_max(
            pos2(pill.left() + 76.0, pill.top() + 8.0),
            pos2(chip_right - chip_width - 10.0, pill.bottom() - 8.0),
        );
        let hint = if listening {
            "Listening...".to_string()
        } else if let Some(line) = self
            .assistant
            .demo
            .as_ref()
            .and_then(demo::Demo::speaking_line)
            .map(str::to_string)
        {
            paint::elide(&line, 70)
        } else if working {
            self.assistant
                .feed
                .latest_did()
                .map_or_else(|| "Working on it...".to_string(), |step| paint::elide(step, 70))
        } else {
            "Ask anything".to_string()
        };
        self.paint_pill_field(ui, field, id, &hint, listening || speaking, working || speaking)
    }

    /// The text field of the pill, with a waveform while someone speaks.
    fn paint_pill_field(
        &mut self,
        ui: &mut Ui,
        field: Rect,
        id: Id,
        hint: &str,
        voice: bool,
        busy: bool,
    ) -> Option<Action> {
        let ctx = ui.ctx().clone();
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(field)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let mut text = std::mem::take(&mut self.assistant.summon.text);
        let wave_room = if voice { 64.0 } else { 0.0 };
        if wave_room > 0.0 {
            waveform(
                &mut child,
                true,
                self.assistant.demo.as_ref().and_then(demo::Demo::level),
            );
        }
        let response = child.add(
            TextEdit::singleline(&mut text)
                .id(id)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .desired_width(field.width() - wave_room - 4.0)
                .font(FontId::proportional(16.0))
                .text_color(theme::FG())
                .hint_text(RichText::new(hint).color(if busy { theme::FG_SOFT() } else { theme::FG_DIM() })),
        );
        if std::mem::take(&mut self.assistant.summon.focus_requested) {
            response.request_focus();
        }
        self.assistant.summon.text = text;
        let listed = command_bar::suggestions(&self.assistant.summon.text, self.assistant.settings.agent);
        let key = self.summon_keys(&ctx, &listed, id);
        if matches!(key, Some(Action::Run(_))) {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        key
    }

    // ---- the sheet ---------------------------------------------------------------------

    fn dock_sheet(&mut self, ctx: &Context) {
        let (canvas, anchor) = self.dock_anchor(ctx);
        let width = SHEET_WIDTH.min(canvas.width() - 48.0).max(420.0);
        let height = SHEET_HEIGHT.min(canvas.height() - 48.0).max(360.0);
        let tiles: Vec<Tile> = self
            .board
            .workspaces
            .iter()
            .map(|workspace| Tile {
                local_id: workspace.local_id.clone(),
                name: workspace.name.clone(),
                panels: Vec::new(),
                agents: 0,
                working: 0,
                needs_you: 0,
            })
            .collect();
        let mut action = None;
        let area = Area::new(Id::new("assistant_dock"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(anchor)
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
                        ui.set_width(width - 36.0);
                        ui.set_height(height - 36.0);
                        self.dock_header(ui, &mut action);
                        ui.add_space(12.0);
                        let body = (ui.available_height() - 62.0 - 1.0 - 46.0 - 8.0).max(120.0);
                        self.conversation(ui, body, false);
                        ui.add_space(8.0);
                        self.desk_prompt(ui, &mut action);
                        super::summon_divider(ui);
                        self.desk_footer(ui, &mut action, true);
                    });
            });
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        self.render_scope_popup(ctx, &tiles);
        match action {
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            Some(Action::Complete(name)) => self.assistant.summon.text = format!("/{name}"),
            Some(Action::Close) => {
                self.assistant.summon.expanded = false;
                self.assistant.summon.focus_requested = true;
            }
            Some(Action::ToggleAsk) => self.run_local_command(LocalCommand::ToggleAsk),
            Some(Action::ToggleDictation) => self.toggle_assistant_dictation(ctx),
            Some(Action::Scope(anchor)) => {
                self.assistant.scope_open = !self.assistant.scope_open;
                self.assistant.scope_anchor = Some(anchor);
            }
            Some(Action::OpenAsChat) | None => {}
        }
    }

    /// The mark and what is going on on the left, the choice of view and the way out on the right.
    fn dock_header(&mut self, ui: &mut Ui, action: &mut Option<Action>) {
        let (status, color) = self.feed_status();
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            icons::paint_mark(ui.painter(), mark);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("Assistant").size(15.0).strong().color(theme::FG()));
                ui.label(RichText::new(status).size(11.5).color(color));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chevron_down(ui) {
                    *action = Some(Action::Close);
                }
                ui.add_space(8.0);
                let raw = self.assistant.summon.raw;
                let style = self.assistant.summon.feed_style;
                let choice = segmented(
                    ui,
                    &[
                        ("Cards", !raw && style == FeedStyle::Cards),
                        ("Chat", !raw && style == FeedStyle::Chat),
                        ("Terminal", raw),
                    ],
                );
                match choice {
                    Some(0) => {
                        self.assistant.summon.feed_style = FeedStyle::Cards;
                        self.assistant.summon.raw = false;
                    }
                    Some(1) => {
                        self.assistant.summon.feed_style = FeedStyle::Chat;
                        self.assistant.summon.raw = false;
                    }
                    Some(_) => self.assistant.summon.raw = true,
                    None => {}
                }
            });
        });
    }
}

/// A key cap with the shortcut, to the left of `right`; returns its width.
fn paint_key_chip(ui: &Ui, right_centre: egui::Pos2, text: &str) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), theme::FG_DIM());
    let size = vec2(galley.size().x + 18.0, 24.0);
    let rect = Rect::from_min_size(pos2(right_centre.x - size.x, right_centre.y - size.y / 2.0), size);
    ui.painter().rect(
        rect,
        CornerRadius::same(8),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    ui.painter().galley(
        rect.left_top() + vec2(9.0, (size.y - galley.size().y) / 2.0),
        galley,
        theme::FG_DIM(),
    );
    size.x
}

/// A row of choices as one control; returns the index pressed. Laid out right to left, so the
/// options are given left to right and drawn in that order.
fn segmented(ui: &mut Ui, options: &[(&str, bool)]) -> Option<usize> {
    let mut pressed = None;
    for (index, (label, selected)) in options.iter().enumerate().rev() {
        let button = egui::Button::new(RichText::new(*label).size(12.0).color(if *selected {
            theme::FG()
        } else {
            theme::FG_DIM()
        }))
        .fill(if *selected {
            theme::ACCENT().gamma_multiply(0.24)
        } else {
            theme::PANEL_BG_ALT()
        })
        .stroke(Stroke::new(
            1.0,
            if *selected {
                theme::ACCENT().gamma_multiply(0.7)
            } else {
                theme::BORDER_SUBTLE()
            },
        ))
        .corner_radius(CornerRadius::same(9))
        .min_size(vec2(0.0, 28.0));
        if ui.add(button).clicked() {
            pressed = Some(index);
        }
        ui.add_space(-2.0);
    }
    pressed
}

/// A chevron pointing down: put it away.
fn chevron_down(ui: &mut Ui) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    if response.hovered() {
        ui.painter().circle_filled(rect.center(), 15.0, theme::PANEL_BG_ALT());
    }
    let c = rect.center();
    ui.painter().add(egui::Shape::line(
        vec![c + vec2(-6.0, -3.0), c + vec2(0.0, 3.0), c + vec2(6.0, -3.0)],
        Stroke::new(1.8, theme::FG_SOFT()),
    ));
    response.on_hover_text("Put it away").clicked()
}
