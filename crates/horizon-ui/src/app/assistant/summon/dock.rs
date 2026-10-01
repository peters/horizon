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

mod concierge;
mod inbox;
mod layouts;
mod lens;
mod messages;
mod mission;
mod scope;

pub(in crate::app::assistant) use layouts::{Channel, Layout3, Pick};
pub(in crate::app::assistant) use messages::{Store as MessageStore, Trust, View as MessageView};

use egui::{
    Align, Context, CornerRadius, FontId, Frame, Id, Layout, Margin, Rect, RichText, Sense, Stroke, TextEdit, Ui, vec2,
};

use super::super::{command_bar, icons};
use super::desk_bar::Tile;
use super::mini::MiniAction;
use super::turns::FeedStyle;
use super::{Action, HorizonApp, demo, waveform};
use crate::app::assistant::command_bar::LocalCommand;
use crate::theme;

/// Which of the three designs the dock is drawn as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum DockStyle {
    /// One conversation; the assistant stands between you and your agents.
    #[default]
    Concierge,
    /// The assistant delegates; you steer a fleet from a board.
    Mission,
    /// The assistant lives on the canvas, next to the work.
    Lens,
}

impl DockStyle {
    fn from_env() -> Self {
        match std::env::var("HORIZON_ASSISTANT_DOCK").as_deref() {
            Ok("mission") => Self::Mission,
            Ok("lens") => Self::Lens,
            _ => Self::Concierge,
        }
    }
}

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
        // A change of engine closes the running agent; the next line starts the new one.
        self.close_assistant_if_restarting();
        self.ensure_assistant_panel(ctx);
        self.follow_assistant_transcript();
        if let Some(index) = self.assistant.summon.pending_workspace.take()
            && let Some(id) = self.board.workspaces.get(index).map(|workspace| workspace.id)
        {
            self.focus_workspace_visible(ctx, id, false);
        }
        self.follow_scope();
        if let Some(id) = self.assistant.summon.pending_reveal.take() {
            self.reveal_selected_panel(ctx, id);
        }
        let style = *self.assistant.summon.dock_style.get_or_insert_with(DockStyle::from_env);
        match (style, self.assistant.summon.expanded) {
            (DockStyle::Concierge, false) => self.concierge_mini(ctx),
            (DockStyle::Concierge, true) => self.concierge_panel(ctx),
            (DockStyle::Mission, false) => self.mission_mini(ctx),
            (DockStyle::Mission, true) => self.mission_board(ctx),
            (DockStyle::Lens, false) => self.lens_mini(ctx),
            (DockStyle::Lens, true) => self.lens_panel(ctx),
        }
        if self.assistant.scope_open {
            let tiles = self.dock_tiles();
            self.render_scope_popup(ctx, &tiles);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    pub(super) fn apply_dock_action(&mut self, ctx: &Context, action: &MiniAction) {
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
            MiniAction::Drag(delta) => self.assistant.summon.mini_offset += delta,
            MiniAction::Go(_) | MiniAction::Style(_) => {}
        }
    }

    /// The capsule. Returns what a key press in its field asked for.
    pub(super) fn paint_pill_field(
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

    pub(super) fn dock_tiles(&self) -> Vec<Tile> {
        self.board
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
            .collect()
    }

    /// What a press in an expanded dock asked for.
    pub(super) fn apply_sheet_action(&mut self, ctx: &Context, action: Option<Action>) {
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
    pub(super) fn dock_header(&mut self, ui: &mut Ui, action: &mut Option<Action>) {
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
                self.view_choice(ui);
            });
        });
    }

    /// Cards, Chat or Terminal, as one control. Laid out right to left.
    fn view_choice(&mut self, ui: &mut Ui) {
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
    }
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
pub(super) fn chevron_down(ui: &mut Ui) -> bool {
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
