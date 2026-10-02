//! The summon overlay: a prompt that floats over the canvas, opened from
//! anywhere with a shortcut. It types into the same hosted agent as the drawer,
//! shows the assistant's plan while it works, and hands over to the drawer
//! ("Open as chat") when the person wants the whole conversation.

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Modifiers, Order, Rect, RichText,
    Sense, Shadow, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2,
};
use horizon_core::PanelKind;

use super::HorizonApp;
use super::command_bar::{self, Command, Entry, LocalCommand, Route};
use super::icons::{self, Icon};
use super::plan;
use crate::app::speech::MicState;
use crate::theme;

const WIDTH: f32 = 780.0;
const INPUT_ID: &str = "assistant_summon_input";
const BARS: [f32; 10] = [6.0, 12.0, 20.0, 10.0, 16.0, 8.0, 14.0, 22.0, 9.0, 5.0];
const HISTORY: usize = 50;
/// Mic button plus the frame's padding above and below it.
const PROMPT_ROW_HEIGHT: f32 = 62.0;

mod deck;
pub(super) mod demo;
mod desk_bar;
mod desk_windows;
mod dock;
pub(super) mod feed;
mod hub;
mod mini;
mod turns;

pub(in crate::app) use desk_bar::ExpandStyle;
pub(in crate::app) use dock::enabled as dock_enabled;
pub(in crate::app) use hub::{HubPage, HubStyle};
pub(in crate::app) use mini::MiniStyle;
pub(in crate::app) use turns::FeedStyle;

#[derive(Default)]
pub(super) struct Summon {
    open: bool,
    text: String,
    selected: usize,
    /// Lines sent from here, oldest first, for the up arrow.
    history: Vec<String>,
    /// Which history line the field currently shows.
    recalled: Option<usize>,
    focus_requested: bool,
    /// Where the overlay was drawn last frame, so the canvas can leave clicks there alone.
    rect: Option<Rect>,
    /// The bar shows the whole conversation, not just the prompt (desk mode).
    expanded: bool,
    style: ExpandStyle,
    /// Show the assistant's raw terminal instead of the feed.
    raw: bool,
    /// How the feed shows the conversation: as a chat, or as a card for each ask.
    feed_style: FeedStyle,
    /// Turns the person has dismissed from the deck, so only newer ones show.
    deck_dismissed: usize,
    /// A workspace the script asked to go to, for the dock to carry out.
    pending_workspace: Option<usize>,
    /// A panel the script asked to reveal, for the dock to carry out.
    pending_reveal: Option<horizon_core::PanelId>,
    /// Which design the dock is drawn as; the environment decides until one is chosen.
    dock_style: Option<dock::DockStyle>,
    /// The scope follows the workspace the person is in.
    scope_follow: bool,
    /// Where the expanded concierge is and how big, once it has been moved or resized.
    sheet: Option<Rect>,
    /// How far the mini dock was dragged from its place.
    mini_offset: egui::Vec2,
    /// A size the script asked the sheet to take; zero means back to the default.
    sheet_request: Option<[f32; 2]>,
    /// The messaging mock: handles, channels, threads and an inbox.
    msgs: dock::MessageStore,
    /// How the text agent sits beside the voice assistant.
    layout: dock::Layout3,
    /// The share of the width the voice side takes in the split layout.
    split: f32,
    /// The thread rail in the split layout.
    rail_open: bool,
    /// The terminal drawer of the feed layout.
    drawer_open: bool,
    /// The modes the text agent can be put in are shown in its row.
    modes_open: bool,
    /// YOLO was pressed once; a second press confirms it.
    yolo_armed: bool,
    /// The agent that carries out an approved plan, when it is not the one that made it.
    plan_executor: Option<horizon_core::PanelKind>,
    /// A message for the text agent once it is running again: the approved plan.
    pending_send: Option<(String, std::time::Instant)>,
    drawer: f32,
    /// Where the composer sends, in the feed layout.
    channel: dock::Channel,
    /// Who the roster layout shows.
    pick: dock::Pick,
    /// The board shows the conversation beside the columns.
    board_chat: bool,
    /// The bar is shrunk to rest above the dock, in this design.
    mini: Option<MiniStyle>,
    /// The design mini mode returns to.
    mini_style: MiniStyle,
    /// Quick nav, hosts, cloud, sessions and settings.
    hub: hub::Hub,
    /// Where the bar's window is on the monitor: left, top, width, height.
    bar_rect: Option<[f32; 4]>,
    /// The window size last asked for, so the window is only resized when it changes.
    window_size: Option<[f32; 2]>,
}

enum Action {
    Run(Entry),
    Complete(&'static str),
    OpenAsChat,
    Close,
    ToggleAsk,
    ToggleDictation,
    /// Open or close the scope picker, anchored at this chip.
    Scope(Rect),
}

impl Summon {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    fn remember(&mut self, line: &str) {
        if self.history.last().is_none_or(|last| last != line) {
            self.history.push(line.to_string());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.recalled = None;
    }

    /// Steps through the history: up goes older, down goes newer and ends on an empty field.
    fn recall(&mut self, older: bool) {
        let next = match (self.recalled, older) {
            (None, true) => self.history.len().checked_sub(1),
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) if index + 1 < self.history.len() => Some(index + 1),
            _ => None,
        };
        self.recalled = next;
        self.text = next
            .and_then(|index| self.history.get(index))
            .cloned()
            .unwrap_or_default();
    }
}

impl HorizonApp {
    /// Opens the overlay and gives it the keyboard, or closes it.
    pub(in crate::app) fn summon_assistant(&mut self) {
        if self.assistant.summon.open {
            self.close_summon();
        } else {
            self.assistant.summon.open = true;
            self.assistant.summon.focus_requested = true;
            self.assistant.summon.recalled = None;
            self.claim_keyboard_for_bar();
        }
    }

    /// Whether the overlay covers this point, so the canvas ignores the pointer there.
    pub(in crate::app) fn assistant_summon_covers(&self, position: egui::Pos2) -> bool {
        self.assistant.summon.open && self.assistant.summon.rect.is_some_and(|rect| rect.contains(position))
    }

    fn close_summon(&mut self) {
        self.assistant.summon.open = false;
        self.assistant.summon.rect = None;
        self.release_assistant_focus();
    }

    /// The dock replaces this bar: its shortcut focuses the dock's field instead.
    fn summon_yields_to_dock(&mut self) -> bool {
        if !dock::enabled() {
            return false;
        }
        if self.assistant.summon.open {
            self.assistant.summon.open = false;
            self.assistant.summon.focus_requested = true;
        }
        true
    }

    /// Draws the overlay above the canvas while it is open.
    pub(in crate::app) fn render_summon(&mut self, ctx: &egui::Context) {
        if self.summon_yields_to_dock()
            || !self.assistant.summon.is_open()
            || self.settings.is_some()
            || self.fullscreen_panel.is_some()
        {
            return;
        }
        // The agent has to be running for the prompt to reach it, drawer or not.
        self.ensure_assistant_panel(ctx);

        let canvas = self.canvas_rect(ctx);
        let width = WIDTH.min(canvas.width() - 48.0).max(360.0);
        let centre_x = canvas.center().x;
        let id = Id::new(INPUT_ID);
        let agent = self.assistant.settings.agent;
        let listed = command_bar::suggestions(&self.assistant.summon.text, agent);
        let mut action = self.summon_keys(ctx, &listed, id);

        let steps = self.assistant.plan.clone();
        let mic = self.summon_mic_state();
        let ask = self.assistant.settings.ask_before_send;
        let feedback = self.assistant.command.feedback_text();
        let mut text = std::mem::take(&mut self.assistant.summon.text);
        let selected = self.assistant.summon.selected;
        let focus_requested = std::mem::take(&mut self.assistant.summon.focus_requested);
        let open_shortcut = self
            .shortcuts
            .summon_assistant
            .display_label(crate::app::util::primary_shortcut_label());

        let area = egui::Area::new(Id::new("assistant_summon"))
            .order(Order::Tooltip)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(pos2(centre_x, canvas.max.y - 96.0))
            .show(ctx, |ui| {
                ui.set_width(width);
                let frame = Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(CornerRadius::same(18))
                    .shadow(Shadow {
                        offset: [0, 24],
                        blur: 60,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .show(ui, |ui| {
                        ui.set_width(width);
                        self.summon_prompt_row(ui, &mut text, id, mic, focus_requested, &mut action);
                        if !listed.is_empty() {
                            summon_divider(ui);
                            for (index, command) in listed.iter().enumerate() {
                                if command_row(ui, command, index == selected, agent).clicked() {
                                    action = Some(Action::Run(entry_for(*command)));
                                }
                            }
                        } else if !steps.is_empty() {
                            summon_divider(ui);
                            Frame::new().inner_margin(Margin::symmetric(18, 4)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                plan::draw_steps(ui, &steps);
                            });
                        }
                        summon_divider(ui);
                        Self::summon_footer(ui, agent, ask, &steps, &mut action);
                    });
                // A faint ring marks the overlay as floating above the canvas.
                ui.painter().rect_stroke(
                    frame.response.rect.expand(3.0),
                    CornerRadius::same(21),
                    Stroke::new(6.0, theme::ACCENT().gamma_multiply(0.08)),
                    StrokeKind::Outside,
                );
                ui.add_space(14.0);
                let hint = feedback.unwrap_or_else(|| {
                    format!("Up arrow for history     Tab to continue in the chat panel     {open_shortcut} to summon from anywhere")
                });
                ui.vertical_centered(|ui| {
                    // A backdrop keeps the hints readable over whatever the canvas shows.
                    Frame::new()
                        .fill(theme::PANEL_BG().gamma_multiply(0.92))
                        .corner_radius(CornerRadius::same(9))
                        .inner_margin(Margin::symmetric(12, 5))
                        .show(ui, |ui| {
                            ui.label(RichText::new(hint).size(12.0).color(theme::FG_DIM()));
                        });
                });
            });
        self.assistant.summon.text = text;
        // Canvas panels raise themselves when clicked; the overlay stays above them.
        ctx.move_to_top(area.response.layer_id);
        self.assistant.summon.rect = Some(area.response.rect);
        if ctx.memory(|memory| memory.has_focus(id)) && self.board.focused.is_some() {
            // A click behind the field must not leave a panel typing alongside it.
            self.claim_keyboard_for_bar();
        }

        match action {
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            Some(Action::Complete(name)) => self.assistant.summon.text = format!("/{name}"),
            Some(Action::OpenAsChat) => self.open_summon_as_chat(),
            Some(Action::Close) => self.close_summon(),
            Some(Action::ToggleAsk) => {
                self.run_local_command(LocalCommand::ToggleAsk);
            }
            Some(Action::ToggleDictation) => self.toggle_assistant_dictation(ctx),
            Some(Action::Scope(_)) | None => {}
        }
    }

    fn summon_prompt_row(
        &self,
        ui: &mut Ui,
        text: &mut String,
        id: Id,
        mic: Option<MicState>,
        focus: bool,
        action: &mut Option<Action>,
    ) {
        // Behind the widgets, so a click on the padding still puts the caret in the field.
        let backdrop = ui.interact(
            Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), PROMPT_ROW_HEIGHT)),
            Id::new("assistant_summon_row"),
            Sense::click(),
        );
        if backdrop.clicked() {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        Frame::new().inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if mic_button(ui, mic).clicked() {
                    *action = Some(Action::ToggleDictation);
                }
                let voice = self.assistant.demo.as_ref().is_some_and(demo::Demo::voice_active);
                waveform(
                    ui,
                    mic == Some(MicState::Recording) || voice,
                    self.assistant.demo.as_ref().and_then(demo::Demo::level),
                );
                let room = (ui.available_width() - 64.0).max(80.0);
                let response = ui.add(
                    TextEdit::singleline(text)
                        .id(id)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .desired_width(room)
                        .font(FontId::proportional(15.0))
                        .text_color(theme::FG())
                        .hint_text(
                            RichText::new(if self.assistant.summon.mini.is_some() {
                                "Ask anything"
                            } else {
                                "Ask the assistant, or type / for commands"
                            })
                            .color(theme::FG_DIM()),
                        ),
                );
                if focus {
                    response.request_focus();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill(ui, "esc").clicked() {
                        *action = Some(Action::Close);
                    }
                });
            });
        });
    }

    fn summon_footer(
        ui: &mut Ui,
        agent: PanelKind,
        ask: bool,
        steps: &[horizon_core::browser::manifest::agent_panels::PlanStep],
        action: &mut Option<Action>,
    ) {
        Frame::new()
            .fill(theme::PANEL_BG())
            .corner_radius(CornerRadius {
                nw: 0,
                ne: 0,
                sw: 18,
                se: 18,
            })
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (mark, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    icons::paint_mark(ui.painter(), mark);
                    let has_tools = matches!(agent, PanelKind::Claude | PanelKind::Codex | PanelKind::Grok);
                    let (label, dot) = if has_tools {
                        ("Horizon tools", theme::PALETTE_GREEN())
                    } else {
                        ("Terminal only", theme::FG_DIM())
                    };
                    chip(ui, label, Some(dot), None);
                    let shield = chip(
                        ui,
                        if ask {
                            "Ask before sending"
                        } else {
                            "Send without asking"
                        },
                        None,
                        Some(Icon::Shield),
                    );
                    if shield
                        .on_hover_text("Click to change whether the assistant asks before typing into other agents")
                        .clicked()
                    {
                        *action = Some(Action::ToggleAsk);
                    }
                    if !steps.is_empty() {
                        ui.label(RichText::new(plan::progress(steps)).size(11.5).color(theme::FG_DIM()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ghost_button(ui, "Open as chat").clicked() {
                            *action = Some(Action::OpenAsChat);
                        }
                    });
                });
            });
    }

    /// The footer of the desk bar: tools, approval, scope and the expand toggle.
    /// Returns the scope chip's rectangle, which the picker opens from.
    fn summon_footer_with_scope(
        &self,
        ui: &mut Ui,
        agent: PanelKind,
        ask: bool,
        steps: &[horizon_core::browser::manifest::agent_panels::PlanStep],
        action: &mut Option<Action>,
        expanded: bool,
    ) -> Rect {
        let mut chip_rect = Rect::NOTHING;
        Frame::new()
            .fill(theme::PANEL_BG())
            .inner_margin(Margin::symmetric(4, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (mark, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    icons::paint_mark(ui.painter(), mark);
                    let has_tools = matches!(agent, PanelKind::Claude | PanelKind::Codex | PanelKind::Grok);
                    let (label, dot) = if has_tools {
                        ("Horizon tools", theme::PALETTE_GREEN())
                    } else {
                        ("Terminal only", theme::FG_DIM())
                    };
                    chip(ui, label, Some(dot), None);
                    let scope = chip(ui, &self.scope_label(), None, Some(Icon::Bot));
                    chip_rect = scope.rect;
                    if scope
                        .on_hover_text("Choose which workspaces the assistant looks at")
                        .clicked()
                    {
                        *action = Some(Action::Scope(chip_rect));
                    }
                    let shield = chip(
                        ui,
                        if ask {
                            "Ask before sending"
                        } else {
                            "Send without asking"
                        },
                        None,
                        Some(Icon::Shield),
                    );
                    if shield
                        .on_hover_text("Click to change whether the assistant asks before typing into other agents")
                        .clicked()
                    {
                        *action = Some(Action::ToggleAsk);
                    }
                    if !steps.is_empty() {
                        ui.label(RichText::new(plan::progress(steps)).size(11.5).color(theme::FG_DIM()));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let toggle = if expanded { "Collapse" } else { "Expand" };
                        if ghost_button(ui, toggle).clicked() {
                            *action = Some(Action::OpenAsChat);
                        }
                    });
                });
            });
        chip_rect
    }

    /// Keys the overlay owns while its field has the caret.
    fn summon_keys(&mut self, ctx: &egui::Context, listed: &[Command], id: Id) -> Option<Action> {
        if !ctx.memory(|memory| memory.has_focus(id)) {
            return None;
        }
        let none = Modifiers::NONE;
        let (down, up, tab, escape, enter) = ctx.input_mut(|input| {
            (
                input.consume_key(none, Key::ArrowDown),
                input.consume_key(none, Key::ArrowUp),
                input.consume_key(none, Key::Tab),
                input.consume_key(none, Key::Escape),
                input.consume_key(none, Key::Enter),
            )
        });
        let summon = &mut self.assistant.summon;
        let last = listed.len().saturating_sub(1);
        summon.selected = summon.selected.min(last);
        if !listed.is_empty() {
            if down {
                summon.selected = (summon.selected + 1).min(last);
            }
            if up {
                summon.selected = summon.selected.saturating_sub(1);
            }
        } else if up {
            summon.recall(true);
        } else if down && summon.recalled.is_some() {
            summon.recall(false);
        }
        if tab {
            return Some(match listed.get(summon.selected) {
                Some(command) => Action::Complete(command.name),
                None => Action::OpenAsChat,
            });
        }
        if escape {
            if summon.text.starts_with('/') {
                summon.text.clear();
                return None;
            }
            return Some(Action::Close);
        }
        if enter {
            return Some(match listed.get(summon.selected) {
                Some(command) if summon.text.starts_with('/') => Action::Run(entry_for(*command)),
                _ => Action::Run(command_bar::parse(&summon.text)),
            });
        }
        None
    }

    /// Runs what was entered; a used line is remembered and clears the field.
    fn submit_summon(&mut self, entry: &Entry) {
        let line = self.assistant.summon.text.trim().to_string();
        if self.run_command(entry) {
            if !matches!(entry, Entry::Local(_)) {
                self.assistant.summon.remember(&line);
                let voice = self.assistant.demo.as_ref().is_some_and(demo::Demo::voice_active);
                if !self.assistant.follow.active() {
                    self.assistant.feed.you(&line, voice);
                }
            }
            self.assistant.summon.text.clear();
            self.assistant.summon.selected = 0;
        }
    }

    /// Hands the line to the drawer's command bar and opens the drawer.
    fn open_summon_as_chat(&mut self) {
        let text = std::mem::take(&mut self.assistant.summon.text);
        self.assistant.summon.open = false;
        self.assistant.command.continue_with(text);
        if !self.assistant.open {
            self.assistant.open = true;
        }
    }

    fn summon_mic_state(&self) -> Option<MicState> {
        if self.assistant.demo.as_ref().is_some_and(demo::Demo::is_listening) {
            return Some(MicState::Recording);
        }
        let panel = self.board.assistant_panel()?;
        match self.speech.as_ref() {
            Some(speech) => Some(speech.mic_state_for(panel)),
            // Desk mode shows an available mic even without a speech engine.
            None if self.desk_mode() => Some(MicState::Idle),
            None => None,
        }
    }

    /// Dictation goes to the assistant's own prompt, the same as the mic on any agent panel.
    fn toggle_assistant_dictation(&mut self, ctx: &egui::Context) {
        let Some(panel) = self.board.assistant_panel() else {
            return;
        };
        if let Some(speech) = self.speech.as_mut() {
            speech.toggle(panel);
            ctx.request_repaint();
        }
    }
}

fn entry_for(command: Command) -> Entry {
    match command.route {
        Route::Local(local) => Entry::Local(local),
        Route::Forward => Entry::Forward(format!("/{}", command.name)),
    }
}

fn summon_divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, theme::BORDER_SUBTLE());
}

fn mic_button(ui: &mut Ui, mic: Option<MicState>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
    let (fill, tip) = match mic {
        None => (theme::BORDER_STRONG(), "Dictation is not set up"),
        Some(MicState::Idle) => (theme::ACCENT(), "Dictate into the assistant"),
        Some(MicState::Recording) => (theme::PALETTE_RED(), "Stop dictation"),
        Some(MicState::Busy) => (theme::FG_DIM(), "Transcribing"),
    };
    ui.painter().circle_filled(rect.center(), 19.0, fill);
    icons::paint(
        ui.painter(),
        rect.center(),
        18.0,
        Icon::Mic,
        Color32::from_rgb(7, 16, 31),
    );
    response.on_hover_text(tip)
}

fn waveform(ui: &mut Ui, live: bool, level: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(super::num::count(BARS.len()) * 6.0, 24.0), Sense::hover());
    let time = super::num::seconds(ui);
    for (index, height) in BARS.iter().enumerate() {
        let scale = match (live, level) {
            (true, Some(level)) => {
                0.18 + 0.95 * level * (0.55 + 0.45 * (time * 9.0 + super::num::count(index) * 1.7).sin().abs())
            }
            (true, None) => 0.55 + 0.45 * (time * 7.0 + super::num::count(index) * 0.9).sin().abs(),
            _ => 1.0,
        };
        let height = height * scale;
        let x = rect.left() + super::num::count(index) * 6.0 + 1.5;
        let bar = Rect::from_center_size(pos2(x + 1.5, rect.center().y), vec2(3.0, height));
        let color = if live {
            theme::ACCENT()
        } else {
            theme::ACCENT().gamma_multiply(0.45)
        };
        ui.painter().rect_filled(bar, CornerRadius::same(2), color);
    }
    if live {
        ui.ctx().request_repaint();
    }
}

fn pill(ui: &mut Ui, text: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), theme::FG_DIM());
    let size = galley.size() + vec2(18.0, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.left_top() + vec2(9.0, 4.0), galley, theme::FG_DIM());
    response
}

fn chip(ui: &mut Ui, text: &str, dot: Option<Color32>, icon: Option<Icon>) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), theme::FG_SOFT());
    let lead = if dot.is_some() || icon.is_some() { 18.0 } else { 0.0 };
    let size = galley.size() + vec2(20.0 + lead, 8.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() {
        theme::BG_ELEVATED()
    } else {
        theme::PANEL_BG_ALT()
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        fill,
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    let mid = rect.left_center() + vec2(15.0, 0.0);
    if let Some(dot) = dot {
        ui.painter().circle_filled(mid, 3.0, dot);
    }
    if let Some(icon) = icon {
        icons::paint(ui.painter(), mid, 12.0, icon, theme::FG_SOFT());
    }
    ui.painter()
        .galley(rect.left_top() + vec2(10.0 + lead, 4.0), galley, theme::FG_SOFT());
    response
}

fn ghost_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.0).color(theme::FG_SOFT()))
            .fill(theme::PANEL_BG_ALT())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 28.0)),
    )
}

fn command_row(ui: &mut Ui, command: &Command, selected: bool, agent: PanelKind) -> egui::Response {
    let agent_name = horizon_core::agent_definition(agent).map_or("Agent", |agent| agent.display_name);
    command_bar::command_row(ui, command, selected, agent_name)
}

#[cfg(test)]
mod tests;
