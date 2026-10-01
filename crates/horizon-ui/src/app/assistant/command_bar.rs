//! The command bar docked at the bottom of the drawer.
//!
//! Plain text is typed into the hosted agent as a message. A leading `/` opens a
//! command list: a few commands are Horizon's own (new thread, engine, threads,
//! approvals), and everything else is forwarded to the agent as typed, so the
//! agent's own slash commands work without Horizon having to know them all.

use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Modifiers, Order, Rect, RichText, Sense,
    Shadow, Stroke, TextEdit, Ui, UiBuilder, vec2,
};
use horizon_core::PanelKind;
use horizon_core::browser::manifest::agent_panels;

use super::HorizonApp;
use super::icons::{self, Icon};
use crate::theme;

pub(super) const BAR_HEIGHT: f32 = 48.0;
pub(super) const BAR_GAP: f32 = 8.0;
const FEEDBACK_FOR: Duration = Duration::from_secs(3);
const MAX_SUGGESTIONS: usize = 10;
const BAR_ID: &str = "assistant_command_bar";

/// What a Horizon-owned command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LocalCommand {
    NewThread,
    Threads,
    Engine,
    ToggleAsk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Local(LocalCommand),
    /// Typed into the agent as written.
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Command {
    /// Without the leading slash.
    pub name: &'static str,
    pub hint: &'static str,
    pub route: Route,
}

const fn local(name: &'static str, hint: &'static str, command: LocalCommand) -> Command {
    Command {
        name,
        hint,
        route: Route::Local(command),
    }
}

const fn forward(name: &'static str, hint: &'static str) -> Command {
    Command {
        name,
        hint,
        route: Route::Forward,
    }
}

const HORIZON_COMMANDS: [Command; 4] = [
    local("new", "Start a new thread", LocalCommand::NewThread),
    local("threads", "Switch to another thread", LocalCommand::Threads),
    local("engine", "Change the agent or how it signs in", LocalCommand::Engine),
    local("ask", "Turn approval before sending on or off", LocalCommand::ToggleAsk),
];

const CLAUDE_COMMANDS: [Command; 6] = [
    forward("compact", "Summarise the conversation to free up context"),
    forward("clear", "Clear the conversation"),
    forward("model", "Switch model"),
    forward("resume", "Resume an earlier session"),
    forward("status", "Show version, account and connectivity"),
    forward("help", "List the agent's commands"),
];
const CODEX_COMMANDS: [Command; 5] = [
    forward("model", "Switch model"),
    forward("compact", "Summarise the conversation to free up context"),
    forward("status", "Show the session configuration"),
    forward("diff", "Show the git diff"),
    forward("help", "List the agent's commands"),
];
const GEMINI_COMMANDS: [Command; 4] = [
    forward("clear", "Clear the conversation"),
    forward("stats", "Show session statistics"),
    forward("tools", "List the available tools"),
    forward("help", "List the agent's commands"),
];
const OPENCODE_COMMANDS: [Command; 4] = [
    forward("compact", "Summarise the conversation to free up context"),
    forward("sessions", "Switch session"),
    forward("models", "Switch model"),
    forward("help", "List the agent's commands"),
];
const OTHER_COMMANDS: [Command; 1] = [forward("help", "List the agent's commands")];

/// Commands the hosted agent understands. Anything typed that is not listed is
/// forwarded too, so this is a convenience, not a gate.
const fn agent_commands(agent: PanelKind) -> &'static [Command] {
    match agent {
        PanelKind::Claude => &CLAUDE_COMMANDS,
        PanelKind::Codex => &CODEX_COMMANDS,
        PanelKind::Gemini => &GEMINI_COMMANDS,
        PanelKind::OpenCode => &OPENCODE_COMMANDS,
        _ => &OTHER_COMMANDS,
    }
}

/// What a submitted line means.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Nothing,
    /// Plain text for the agent.
    Message(String),
    Local(LocalCommand),
    /// A slash command for the agent, as written.
    Forward(String),
}

/// Reads a submitted line. Control characters never reach the agent.
pub(super) fn parse(text: &str) -> Entry {
    let clean = agent_panels::printable(text);
    let clean = clean.trim();
    if clean.is_empty() {
        return Entry::Nothing;
    }
    let Some(rest) = clean.strip_prefix('/') else {
        return Entry::Message(clean.to_string());
    };
    if let Some(command) = HORIZON_COMMANDS.iter().find(|command| command.name == rest)
        && let Route::Local(local) = command.route
    {
        return Entry::Local(local);
    }
    Entry::Forward(clean.to_string())
}

/// The commands whose name starts with, or failing that contains, what was typed
/// after the slash. Nothing once the line has an argument.
pub(super) fn suggestions(text: &str, agent: PanelKind) -> Vec<Command> {
    let Some(query) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if query.contains(char::is_whitespace) {
        return Vec::new();
    }
    let query = query.to_lowercase();
    let all = || HORIZON_COMMANDS.iter().chain(agent_commands(agent)).copied();
    let mut found: Vec<Command> = all().filter(|command| command.name.starts_with(&query)).collect();
    found.extend(all().filter(|command| !command.name.starts_with(&query) && command.name.contains(&query)));
    found.dedup_by_key(|command| command.name);
    found.truncate(MAX_SUGGESTIONS);
    found
}

#[derive(Default)]
pub(super) struct CommandBar {
    text: String,
    selected: usize,
    feedback: Option<(String, Instant)>,
}

impl CommandBar {
    pub(super) fn show_feedback(&mut self, message: impl Into<String>) {
        self.feedback = Some((message.into(), Instant::now()));
    }
}

enum BarAction {
    Run(Entry),
    Complete(&'static str),
}

impl HorizonApp {
    /// Draws the bar inside `rect`, with the command list above it while a slash is typed.
    pub(super) fn render_command_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let id = Id::new(BAR_ID);
        let ctx = ui.ctx().clone();
        let has_focus = ctx.memory(|memory| memory.has_focus(id));
        // The terminal and the bar never both own the keyboard.
        if has_focus && self.assistant.focused {
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
        let agent = self.assistant.settings.agent;
        let listed = if has_focus {
            suggestions(&self.assistant.command.text, agent)
        } else {
            Vec::new()
        };
        let mut action = None;
        if has_focus {
            action = self.bar_keys(&ctx, &listed);
        }

        let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
        let mut response = None;
        let mut send_clicked = false;
        Frame::new()
            .fill(theme::BG_ELEVATED())
            .stroke(Stroke::new(
                1.0,
                if has_focus {
                    theme::ACCENT().gamma_multiply(0.7)
                } else {
                    theme::BORDER_SUBTLE()
                },
            ))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(12, 0))
            .show(&mut child, |ui| {
                ui.set_height(rect.height() - 2.0);
                ui.set_width(rect.width() - 26.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new("\u{203a}").size(18.0).strong().color(theme::ACCENT()));
                    let width = (ui.available_width() - 40.0).max(60.0);
                    response = Some(
                        ui.add(
                            TextEdit::singleline(&mut self.assistant.command.text)
                                .id(id)
                                .frame(Frame::NONE)
                                .margin(Margin::ZERO)
                                .desired_width(width)
                                .font(egui::TextStyle::Body)
                                .text_color(theme::FG())
                                .hint_text(
                                    RichText::new("Message the assistant, or type / for commands")
                                        .color(theme::FG_DIM()),
                                ),
                        ),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        send_clicked = send_button(ui, !self.assistant.command.text.trim().is_empty());
                    });
                });
            });
        let Some(response) = response else {
            return;
        };
        if response.gained_focus() {
            self.claim_keyboard_for_bar();
        }
        if send_clicked {
            action = Some(BarAction::Run(parse(&self.assistant.command.text)));
            response.request_focus();
        }
        if response.changed() {
            self.assistant.command.selected = 0;
        }
        // Enter ends the edit; put the caret back so the next line can be typed.
        if response.lost_focus() && ctx.input(|input| input.key_pressed(Key::Enter)) && action.is_none() {
            action = Some(self.entered(&listed));
            response.request_focus();
        }

        if has_focus
            && !listed.is_empty()
            && let Some(chosen) = self.render_command_list(&ctx, rect, &listed)
        {
            action = Some(BarAction::Run(entry_for(chosen)));
            response.request_focus();
        }
        self.render_bar_feedback(&ctx, rect);
        match action {
            Some(BarAction::Run(entry)) => self.run_command(&entry),
            Some(BarAction::Complete(name)) => {
                self.assistant.command.text = format!("/{name}");
            }
            None => {}
        }
    }

    /// Navigation keys, taken before the text field sees them.
    fn bar_keys(&mut self, ctx: &egui::Context, listed: &[Command]) -> Option<BarAction> {
        let (down, up, tab, escape) = ctx.input_mut(|input| {
            let none = Modifiers::NONE;
            (
                !listed.is_empty() && input.consume_key(none, Key::ArrowDown),
                !listed.is_empty() && input.consume_key(none, Key::ArrowUp),
                !listed.is_empty() && input.consume_key(none, Key::Tab),
                input.consume_key(none, Key::Escape),
            )
        });
        let last = listed.len().saturating_sub(1);
        let bar = &mut self.assistant.command;
        bar.selected = bar.selected.min(last);
        if down {
            bar.selected = (bar.selected + 1).min(last);
        }
        if up {
            bar.selected = bar.selected.saturating_sub(1);
        }
        if tab {
            return listed
                .get(bar.selected)
                .map(|command| BarAction::Complete(command.name));
        }
        if escape {
            if bar.text.is_empty() {
                // Nothing to dismiss: hand the keyboard to the terminal.
                ctx.memory_mut(|memory| memory.surrender_focus(Id::new(BAR_ID)));
                self.focus_assistant();
            } else {
                bar.text.clear();
            }
        }
        None
    }

    /// What Enter does: run the highlighted command, or the line as written.
    fn entered(&self, listed: &[Command]) -> BarAction {
        let typed = &self.assistant.command.text;
        match listed.get(self.assistant.command.selected) {
            Some(command) if typed.starts_with('/') => BarAction::Run(entry_for(*command)),
            _ => BarAction::Run(parse(typed)),
        }
    }

    fn render_command_list(&mut self, ctx: &egui::Context, bar: Rect, listed: &[Command]) -> Option<Command> {
        let mut chosen = None;
        let selected = self.assistant.command.selected;
        let agent_name =
            horizon_core::agent_definition(self.assistant.settings.agent).map_or("Agent", |agent| agent.display_name);
        egui::Area::new(Id::new("assistant_command_list"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(bar.left_top() - vec2(0.0, 6.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(6))
                    .shadow(Shadow {
                        offset: [0, -8],
                        blur: 28,
                        spread: 1,
                        color: Color32::from_black_alpha(110),
                    })
                    .show(ui, |ui| {
                        ui.set_width(bar.width() - 14.0);
                        for (index, command) in listed.iter().enumerate() {
                            if command_row(ui, command, index == selected, agent_name).clicked() {
                                chosen = Some(*command);
                            }
                        }
                    });
            });
        chosen
    }

    /// A short line above the bar after a Horizon command ran.
    fn render_bar_feedback(&mut self, ctx: &egui::Context, bar: Rect) {
        let Some((message, shown)) = self.assistant.command.feedback.clone() else {
            return;
        };
        let Some(remaining) = FEEDBACK_FOR.checked_sub(shown.elapsed()) else {
            self.assistant.command.feedback = None;
            return;
        };
        ctx.request_repaint_after(remaining);
        egui::Area::new(Id::new("assistant_command_feedback"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(bar.left_top() - vec2(-4.0, 6.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new(message).size(11.5).color(theme::FG_DIM()));
            });
    }

    /// The bar takes the keyboard from the terminal and from any canvas panel.
    fn claim_keyboard_for_bar(&mut self) {
        self.assistant.focused = false;
        if let Some(panel) = self.board.focused.take() {
            self.assistant.previous_focus = Some(panel);
        }
    }

    /// Carries out a submitted line.
    pub(super) fn run_command(&mut self, entry: &Entry) {
        match entry {
            Entry::Nothing => {}
            Entry::Local(command) => {
                self.assistant.command.text.clear();
                self.run_local_command(*command);
            }
            Entry::Message(text) | Entry::Forward(text) => {
                let Some(panel_id) = self.board.assistant_panel() else {
                    self.assistant
                        .command
                        .show_feedback("The assistant is not running yet.");
                    return;
                };
                if self.send_to_agent(panel_id, text, true, Instant::now()) {
                    self.assistant.command.text.clear();
                } else {
                    self.assistant
                        .command
                        .show_feedback("The assistant cannot take input right now.");
                }
            }
        }
    }

    fn run_local_command(&mut self, command: LocalCommand) {
        match command {
            LocalCommand::NewThread => self.new_assistant_thread(),
            LocalCommand::Threads => self.assistant.thread_menu_open = true,
            LocalCommand::Engine => {
                self.assistant.engine_open = true;
                self.assistant.draft = self.assistant.settings;
            }
            LocalCommand::ToggleAsk => {
                let ask = !self.assistant.settings.ask_before_send;
                self.assistant.settings.ask_before_send = ask;
                self.assistant.draft.ask_before_send = ask;
                if let Err(error) = self.assistant.settings.save(&self.assistant.home) {
                    self.assistant
                        .command
                        .show_feedback(format!("Could not save the setting: {error}"));
                    return;
                }
                self.assistant.command.show_feedback(if ask {
                    "The assistant now asks before typing into other agents."
                } else {
                    "The assistant now types into other agents without asking."
                });
            }
        }
    }
}

fn entry_for(command: Command) -> Entry {
    match command.route {
        Route::Local(local) => Entry::Local(local),
        Route::Forward => Entry::Forward(format!("/{}", command.name)),
    }
}

fn command_row(ui: &mut Ui, command: &Command, selected: bool, agent_name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            theme::ACCENT().gamma_multiply(if selected { 0.16 } else { 0.08 }),
        );
    }
    let painter = ui.painter();
    let left = rect.left_center() + vec2(10.0, 0.0);
    let name = painter.text(
        left,
        Align2::LEFT_CENTER,
        format!("/{}", command.name),
        egui::FontId::monospace(13.0),
        if selected { theme::FG() } else { theme::FG_SOFT() },
    );
    painter.text(
        name.right_center() + vec2(12.0, 0.0),
        Align2::LEFT_CENTER,
        command.hint,
        egui::FontId::proportional(12.0),
        theme::FG_DIM(),
    );
    let (tag, color) = match command.route {
        Route::Local(_) => ("Horizon", theme::ACCENT()),
        Route::Forward => (agent_name, theme::FG_DIM()),
    };
    painter.text(
        rect.right_center() - vec2(10.0, 0.0),
        Align2::RIGHT_CENTER,
        tag,
        egui::FontId::proportional(10.5),
        color,
    );
    response
}

fn send_button(ui: &mut Ui, enabled: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let color = if enabled { theme::ACCENT() } else { theme::FG_DIM() };
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), color.gamma_multiply(0.18));
    }
    icons::paint(ui.painter(), rect.center(), 16.0, Icon::Send, color);
    response.on_hover_text("Send").clicked() && enabled
}

#[cfg(test)]
mod tests;
