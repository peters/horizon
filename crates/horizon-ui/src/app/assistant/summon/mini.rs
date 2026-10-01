//! Mini mode: the command bar shrunk to something that rests just above the dock.
//!
//! Three designs to compare, switched live. All of them keep the three things that
//! matter when nothing is being asked: where the work is (a dot per desktop
//! workspace, coloured by what its agents are doing), whether anything needs the
//! person, and a way to talk. Typing, or a click on the mark, opens the full bar.

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2,
};
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::{icons, num};
use super::desk_bar::{Tile, paint};
use super::feed::AgentRow;
use super::{Action, HorizonApp};
use crate::app::desk::DeskState;
use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum MiniStyle {
    /// A capsule: workspace dots on the left, the prompt on the right.
    #[default]
    Pill,
    /// A thin status bar across the screen, numbered workspaces and a summary.
    Strip,
    /// A floating orb whose ring shows the agents, with a caption that appears when there is news.
    Orb,
    /// The orb with the news as cards stacked above it: what was done, what is asked, what is under way.
    Deck,
}

impl MiniStyle {
    pub(super) const ALL: [Self; 4] = [Self::Pill, Self::Strip, Self::Orb, Self::Deck];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Pill => "Pill",
            Self::Strip => "Strip",
            Self::Orb => "Orb",
            Self::Deck => "Deck",
        }
    }

    /// Window width and height.
    pub(super) fn size(self, monitor_width: f32) -> [f32; 2] {
        match self {
            Self::Pill => [720.0, 76.0],
            Self::Strip => [(monitor_width - 160.0).clamp(900.0, 1560.0), 54.0],
            Self::Orb | Self::Deck => [600.0, 104.0],
        }
    }

    /// Whether the window is drawn on a plate of its own, or the shapes float on the desktop.
    pub(super) const fn plated(self) -> bool {
        !matches!(self, Self::Orb | Self::Deck)
    }
}

/// The colour a workspace dot gets from what its agents are doing.
fn tile_color(tile: &Tile) -> Color32 {
    if tile.needs_you > 0 {
        theme::PALETTE_RED()
    } else if tile.working > 0 {
        theme::PALETTE_YELLOW()
    } else if tile.agents > 0 {
        theme::PALETTE_GREEN()
    } else if tile.panels.is_empty() {
        theme::BORDER_STRONG()
    } else {
        theme::FG_DIM()
    }
}

pub(super) enum MiniAction {
    Expand,
    /// Open the cards view of the conversation, or its terminal.
    Open {
        terminal: bool,
    },
    /// Hide the deck's cards for every turn up to and including this one.
    Dismiss(usize),
    Go(usize),
    Dictate,
    Style(MiniStyle),
    Answer(horizon_core::PanelId, bool),
}

impl HorizonApp {
    /// The whole window in mini mode.
    pub(super) fn desk_mini(&mut self, ui: &mut Ui, style: MiniStyle, tiles: &[Tile], desk: &DeskState) {
        let mut actions = Vec::new();
        match style {
            MiniStyle::Pill => self.mini_pill(ui, tiles, desk, &mut actions),
            MiniStyle::Strip => self.mini_strip(ui, tiles, desk, &mut actions),
            MiniStyle::Orb => self.mini_orb(ui, &mut actions, false),
            MiniStyle::Deck => self.mini_orb(ui, &mut actions, true),
        }
        for action in actions {
            match action {
                MiniAction::Expand => self.leave_mini(),
                MiniAction::Open { terminal } => {
                    self.leave_mini();
                    self.assistant.summon.expanded = true;
                    self.assistant.summon.style = super::ExpandStyle::Sheet;
                    self.assistant.summon.feed_style = super::FeedStyle::Cards;
                    self.assistant.summon.raw = terminal;
                }
                MiniAction::Dismiss(turn) => self.assistant.summon.deck_dismissed = turn + 1,
                MiniAction::Go(index) => {
                    if let Some(desk) = self.assistant.desk.as_ref() {
                        desk.switch(index);
                    }
                }
                MiniAction::Dictate => self.toggle_assistant_dictation(ui.ctx()),
                MiniAction::Style(style) => {
                    self.assistant.summon.mini_style = style;
                    self.assistant.summon.mini = Some(style);
                }
                MiniAction::Answer(id, allow) => self.answer_agent(id, allow),
            }
        }
        // Typing opens the bar. A scripted dictation does not.
        if !self.assistant.summon.text.is_empty() && self.assistant.demo.is_none() {
            self.leave_mini();
        }
    }

    /// Back to the full bar, with the cursor in the prompt.
    pub(super) fn leave_mini(&mut self) {
        self.assistant.summon.mini = None;
        self.assistant.summon.focus_requested = true;
    }

    // ---- A: pill -----------------------------------------------------------

    fn mini_pill(&mut self, ui: &mut Ui, tiles: &[Tile], desk: &DeskState, actions: &mut Vec<MiniAction>) {
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let mark = Rect::from_center_size(pos2(area.left() + 22.0, area.center().y), vec2(38.0, 38.0));
        if Self::mark_button(ui, mark, tiles, actions) {
            actions.push(MiniAction::Expand);
        }
        let dots_width = 16.0 * num::count(tiles.len()) + 8.0;
        let dots = Rect::from_min_size(pos2(mark.right() + 14.0, area.top()), vec2(dots_width, area.height()));
        Self::workspace_dots(ui, dots, tiles, desk, actions);
        ui.painter().vline(
            dots.right() + 8.0,
            area.y_range().shrink(14.0),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
        );
        let prompt = Rect::from_min_max(pos2(dots.right() + 14.0, area.top() - 6.0), area.right_bottom());
        let mut child = ui.new_child(UiBuilder::new().max_rect(prompt));
        let mut action = None;
        self.desk_prompt(&mut child, &mut action);
        match action {
            Some(Action::ToggleDictation) => actions.push(MiniAction::Dictate),
            Some(Action::Close) => actions.push(MiniAction::Expand),
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            _ => {}
        }
    }

    // ---- B: strip ----------------------------------------------------------

    fn mini_strip(&mut self, ui: &mut Ui, tiles: &[Tile], desk: &DeskState, actions: &mut Vec<MiniAction>) {
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let centre_y = area.center().y;
        let mark = Rect::from_center_size(pos2(area.left() + 18.0, centre_y), vec2(30.0, 30.0));
        if Self::mark_button(ui, mark, tiles, actions) {
            actions.push(MiniAction::Expand);
        }

        // What needs attention, as two chips.
        let (needs, working): (usize, usize) = tiles.iter().fold((0, 0), |(needs, working), tile| {
            (needs + tile.needs_you, working + tile.working)
        });
        let mut x = mark.right() + 14.0;
        let chips = [
            (needs > 0, format!("{needs} needs you"), theme::PALETTE_RED()),
            (working > 0, format!("{working} working"), theme::PALETTE_YELLOW()),
        ];
        let mut any = false;
        for (show, text, color) in chips {
            if show {
                x = status_chip(ui, pos2(x, centre_y), &text, color) + 8.0;
                any = true;
            }
        }
        if !any {
            ui.painter().text(
                pos2(x, centre_y),
                Align2::LEFT_CENTER,
                "All quiet",
                FontId::proportional(12.5),
                theme::FG_DIM(),
            );
        }

        // The workspaces, numbered, in the middle.
        let step = 38.0;
        let width = step * num::count(tiles.len());
        let numbers = Rect::from_center_size(pos2(area.center().x + 40.0, centre_y), vec2(width, 34.0));
        for (index, tile) in tiles.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(numbers.left() + num::count(index) * step, numbers.top()),
                vec2(step - 4.0, 34.0),
            );
            let active = desk.active == index;
            let response = ui.interact(rect, egui::Id::new(("mini_strip_ws", index)), Sense::click());
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(9),
                if active {
                    theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.3)
                } else if response.hovered() {
                    theme::PANEL_BG_ALT()
                } else {
                    Color32::TRANSPARENT
                },
                Stroke::new(1.0, if active { theme::ACCENT() } else { Color32::TRANSPARENT }),
                StrokeKind::Inside,
            );
            painter.text(
                rect.center() - vec2(0.0, 3.0),
                Align2::CENTER_CENTER,
                format!("{}", index + 1),
                FontId::monospace(11.5),
                if active { theme::FG() } else { theme::FG_SOFT() },
            );
            painter.circle_filled(pos2(rect.center().x, rect.bottom() - 6.0), 2.4, tile_color(tile));
            if response.on_hover_text(&tile.name).clicked() {
                actions.push(MiniAction::Go(index));
            }
        }

        // Right: what the assistant is saying, the mic and the way out.
        let mic = Rect::from_center_size(pos2(area.right() - 78.0, centre_y), vec2(34.0, 34.0));
        let expand = Rect::from_center_size(pos2(area.right() - 28.0, centre_y), vec2(34.0, 34.0));
        if round_button(ui, mic, icons::Icon::Mic, "Talk to the assistant", self.speaking_now()).clicked() {
            actions.push(MiniAction::Dictate);
        }
        if chevron_button(ui, expand).clicked() {
            actions.push(MiniAction::Expand);
        }
        if let Some(line) = self.assistant.demo.as_ref().and_then(super::demo::Demo::speaking_line) {
            let room = (mic.left() - numbers.right() - 20.0).max(0.0);
            if room > 120.0 {
                ui.painter().text(
                    pos2(mic.left() - 12.0, centre_y),
                    Align2::RIGHT_CENTER,
                    paint::elide(line, num::index(room / 6.6)),
                    FontId::proportional(12.0),
                    theme::FG(),
                );
            }
        }
    }

    // ---- C: orb --------------------------------------------------------------

    fn mini_orb(&mut self, ui: &mut Ui, actions: &mut Vec<MiniAction>, deck: bool) {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        // The deck keeps its orb in the bottom row and stacks the cards above it.
        let area = if deck {
            Rect::from_min_max(
                pos2(full.left(), full.bottom() - MiniStyle::Deck.size(0.0)[1]),
                full.right_bottom(),
            )
        } else {
            full
        };
        if deck {
            let toasts = self.deck_toasts();
            let stack = Rect::from_min_max(full.left_top(), pos2(full.right(), area.top()));
            self.paint_toasts(ui, stack, &toasts, actions);
        }
        let centre = pos2(area.left() + 52.0, area.center().y);
        let radius = 34.0;
        let agents = self.feed_agents();
        let now = num::seconds(ui);
        self.paint_orb(ui, centre, radius, &agents, now);
        let hit = Rect::from_center_size(centre, vec2(radius * 2.0, radius * 2.0));
        if ui
            .interact(hit, egui::Id::new("mini_orb"), Sense::click())
            .on_hover_text("Open the command bar")
            .clicked()
        {
            actions.push(MiniAction::Expand);
        }
        let caption = Rect::from_min_max(
            pos2(centre.x + radius + 16.0, area.top() + 14.0),
            pos2(area.right() - 2.0, area.bottom() - 14.0),
        );
        self.orb_caption(ui, caption, &agents, now, deck, actions);
    }

    /// The orb: a breathing glow, a ring with an arc for each agent, the mark and a badge for who waits.
    pub(super) fn paint_orb(&self, ui: &mut Ui, centre: Pos2, radius: f32, agents: &[AgentRow], now: f32) {
        let level = self.assistant.demo.as_ref().and_then(super::demo::Demo::level);
        let speaking = self.speaking_now();
        // A soft glow that breathes, and swells with the voice.
        let swell = level.map_or(0.0, |level| level * 14.0);
        for step in 0..6_u8 {
            let grow = f32::from(step) * 3.5 + swell;
            let alpha = if speaking { 44.0 } else { 20.0 } / (1.0 + f32::from(step));
            ui.painter().circle_filled(
                centre,
                radius + 4.0 + grow + 1.5 * (now * 1.6).sin(),
                theme::alpha(theme::ACCENT(), num::alpha_byte(alpha)),
            );
        }
        ui.painter().circle_filled(centre, radius, theme::PANEL_BG());
        ui.painter()
            .circle_stroke(centre, radius, Stroke::new(1.0, theme::ACCENT().gamma_multiply(0.5)));

        // The ring: one arc for each agent, in the colour of what it is doing.
        let ring = radius - 6.0;
        if agents.is_empty() {
            ui.painter()
                .circle_stroke(centre, ring, Stroke::new(3.0, theme::BORDER_STRONG()));
        } else {
            let span = std::f32::consts::TAU / num::count(agents.len());
            let gap = if agents.len() > 1 { 0.32 } else { 0.0 };
            for (index, agent) in agents.iter().enumerate() {
                let (color, pulses) = paint::state_color(agent.state);
                let color = if pulses {
                    color.gamma_multiply(0.7 + 0.3 * (now * 4.0 + num::count(index)).sin().abs())
                } else {
                    color
                };
                let start = -std::f32::consts::FRAC_PI_2 + span * num::count(index) + gap / 2.0;
                let points: Vec<_> = (0..=20_u8)
                    .map(|step| {
                        let angle = start + (span - gap) * f32::from(step) / 20.0;
                        centre + vec2(angle.cos(), angle.sin()) * ring
                    })
                    .collect();
                ui.painter().add(Shape::line(points, Stroke::new(4.0, color)));
            }
        }
        let mark = Rect::from_center_size(centre, vec2(radius * 0.9, radius * 0.9));
        icons::paint_mark(ui.painter(), mark);

        let needs = agents
            .iter()
            .filter(|agent| agent.state == AgentState::NeedsInput)
            .count();
        if needs > 0 {
            let badge = centre + vec2(radius * 0.72, -radius * 0.72);
            ui.painter().circle_filled(badge, 9.0, theme::PALETTE_RED());
            ui.painter()
                .circle_stroke(badge, 9.0, Stroke::new(2.0, theme::PANEL_BG()));
            ui.painter().text(
                badge,
                Align2::CENTER_CENTER,
                format!("{needs}"),
                FontId::proportional(10.5),
                Color32::WHITE,
            );
        }
    }

    /// The caption beside the orb: the question if an agent asks, else what is being said, else the latest news.
    fn orb_caption(
        &self,
        ui: &mut Ui,
        caption: Rect,
        agents: &[AgentRow],
        now: f32,
        deck: bool,
        actions: &mut Vec<MiniAction>,
    ) {
        // In the deck a question is a card above the orb.
        let asking = agents
            .iter()
            .find(|agent| agent.state == AgentState::NeedsInput)
            .filter(|_| !deck);
        let pressed = self.assistant.demo.as_ref().and_then(super::demo::Demo::press_progress);
        if let Some(agent) = asking {
            plate(ui, caption, theme::PALETTE_YELLOW());
            ui.painter().text(
                caption.left_top() + vec2(14.0, 17.0),
                Align2::LEFT_CENTER,
                paint::elide(&format!("{} asks", agent.title), 28),
                FontId::proportional(12.0),
                theme::PALETTE_YELLOW(),
            );
            ui.painter().text(
                caption.left_top() + vec2(14.0, 38.0),
                Align2::LEFT_CENTER,
                paint::elide(&agent.last, 40),
                FontId::monospace(11.5),
                theme::FG(),
            );
            let allow = Rect::from_center_size(pos2(caption.right() - 106.0, caption.center().y), vec2(64.0, 30.0));
            let deny = Rect::from_center_size(pos2(caption.right() - 38.0, caption.center().y), vec2(60.0, 30.0));
            if small_button(ui, allow, "Allow", true).clicked() {
                actions.push(MiniAction::Answer(agent.id, true));
            }
            if let Some(progress) = pressed {
                paint::paint_click(ui, allow.center(), progress);
            }
            if small_button(ui, deny, "Deny", false).clicked() {
                actions.push(MiniAction::Answer(agent.id, false));
            }
            ui.ctx().request_repaint();
        } else if self
            .assistant
            .demo
            .as_ref()
            .is_some_and(super::demo::Demo::is_listening)
        {
            // What is being dictated, as it arrives.
            plate(ui, caption, theme::ACCENT());
            let heard = self.assistant.summon.text.clone();
            let shown: String = heard
                .chars()
                .rev()
                .take(54)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            ui.painter().text(
                caption.left_center() + vec2(16.0, 0.0),
                Align2::LEFT_CENTER,
                if shown.is_empty() {
                    "Listening...".to_string()
                } else {
                    shown
                },
                FontId::proportional(13.0),
                theme::FG(),
            );
            ui.ctx().request_repaint();
        } else if let Some(line) = self.assistant.demo.as_ref().and_then(super::demo::Demo::speaking_line) {
            plate(ui, caption, theme::ACCENT());
            ui.painter().text(
                caption.left_center() + vec2(16.0, 0.0),
                Align2::LEFT_CENTER,
                paint::elide(line, 62),
                FontId::proportional(13.0),
                theme::FG(),
            );
        } else if agents.iter().any(|agent| agent.state == AgentState::Working) {
            let working = agents.iter().filter(|agent| agent.state == AgentState::Working).count();
            plate(ui, caption, theme::PALETTE_YELLOW());
            let dots = num::index(now * 3.0) % 4;
            ui.painter().text(
                caption.left_center() + vec2(16.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{working} agents working{}", ".".repeat(dots)),
                FontId::proportional(13.0),
                theme::FG(),
            );
            ui.ctx().request_repaint();
        } else if let Some(text) = self.assistant.feed.latest_did() {
            plate(ui, caption, theme::BORDER_STRONG());
            ui.painter()
                .circle_filled(caption.left_center() + vec2(16.0, 0.0), 3.5, theme::ACCENT());
            ui.painter().text(
                caption.left_center() + vec2(28.0, 0.0),
                Align2::LEFT_CENTER,
                paint::elide(text, 56),
                FontId::proportional(12.5),
                theme::FG_SOFT(),
            );
        }
    }

    // ---- shared --------------------------------------------------------------

    pub(super) fn speaking_now(&self) -> bool {
        self.assistant
            .demo
            .as_ref()
            .is_some_and(super::demo::Demo::voice_active)
    }

    /// The assistant's mark as a button, with a red dot when someone is waiting for the person.
    fn mark_button(ui: &mut Ui, rect: Rect, tiles: &[Tile], actions: &mut Vec<MiniAction>) -> bool {
        let response = ui.interact(
            rect,
            egui::Id::new(("mini_mark", num::whole(rect.min.x))),
            Sense::click(),
        );
        icons::paint_mark(ui.painter(), rect);
        if tiles.iter().any(|tile| tile.needs_you > 0) {
            let at = rect.right_top() + vec2(-2.0, 2.0);
            ui.painter().circle_filled(at, 5.0, theme::PALETTE_RED());
            ui.painter().circle_stroke(at, 5.0, Stroke::new(1.5, theme::PANEL_BG()));
        }
        response.context_menu(|ui| {
            for style in MiniStyle::ALL {
                if ui.button(style.label()).clicked() {
                    actions.push(MiniAction::Style(style));
                    ui.close();
                }
            }
        });
        response
            .on_hover_text("Open the command bar. Right-click for the other mini designs")
            .clicked()
    }

    /// A dot per workspace, in the colour of what its agents are doing.
    fn workspace_dots(ui: &mut Ui, rect: Rect, tiles: &[Tile], desk: &DeskState, actions: &mut Vec<MiniAction>) {
        let step = (rect.width() / num::count(tiles.len().max(1))).min(18.0);
        let now = num::seconds(ui);
        for (index, tile) in tiles.iter().enumerate() {
            let centre = pos2(rect.left() + step * (num::count(index) + 0.5), rect.center().y);
            let hit = Rect::from_center_size(centre, vec2(step, 30.0));
            let response = ui.interact(hit, egui::Id::new(("mini_dot", index)), Sense::click());
            let color = tile_color(tile);
            let pulse = if tile.working > 0 || tile.needs_you > 0 {
                0.8 + 0.4 * (now * 4.0).sin().abs()
            } else {
                1.0
            };
            let active = desk.active == index;
            let radius = if active { 5.2 } else { 3.6 } * pulse;
            if active {
                ui.painter()
                    .circle_stroke(centre, 9.0, Stroke::new(1.6, theme::ACCENT()));
            }
            ui.painter().circle_filled(centre, radius, color);
            if response
                .on_hover_text(format!("{}  {}", index + 1, tile.name))
                .clicked()
            {
                actions.push(MiniAction::Go(index));
            }
        }
    }
}

// ---- small painters -------------------------------------------------------

/// A rounded plate with a tinted edge, behind the orb's caption.
pub(super) fn plate(ui: &Ui, rect: Rect, edge: Color32) {
    ui.painter().add(
        egui::Shadow {
            offset: [0, 8],
            blur: 26,
            spread: 0,
            color: Color32::from_black_alpha(110),
        }
        .as_shape(rect, CornerRadius::same(20)),
    );
    ui.painter().rect(
        rect,
        CornerRadius::same(20),
        theme::PANEL_BG(),
        Stroke::new(1.2, edge.gamma_multiply(0.7)),
        StrokeKind::Inside,
    );
}

/// A pill with a coloured dot and text; returns where it ends.
fn status_chip(ui: &Ui, left_centre: egui::Pos2, text: &str, color: Color32) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(12.0), color);
    let rect = Rect::from_min_size(left_centre - vec2(0.0, 12.0), vec2(galley.size().x + 30.0, 24.0));
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        color.gamma_multiply(0.15),
        Stroke::new(1.0, color.gamma_multiply(0.5)),
        StrokeKind::Inside,
    );
    ui.painter()
        .circle_filled(rect.left_center() + vec2(12.0, 0.0), 3.2, color);
    ui.painter()
        .galley(rect.left_center() + vec2(22.0, -galley.size().y / 2.0), galley, color);
    rect.right()
}

pub(super) fn small_button(ui: &mut Ui, rect: Rect, text: &str, primary: bool) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(("mini_btn", text)), Sense::click());
    let fill = if primary {
        theme::ACCENT()
    } else if response.hovered() {
        theme::BORDER_SUBTLE()
    } else {
        theme::PANEL_BG_ALT()
    };
    ui.painter().rect_filled(rect, CornerRadius::same(15), fill);
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

pub(super) fn round_button(ui: &mut Ui, rect: Rect, icon: icons::Icon, tip: &str, lit: bool) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new(("mini_round", tip)), Sense::click());
    let fill = if lit {
        theme::ACCENT()
    } else if response.hovered() {
        theme::BORDER_SUBTLE()
    } else {
        theme::PANEL_BG_ALT()
    };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, fill);
    icons::paint(
        ui.painter(),
        rect.center(),
        18.0,
        icon,
        if lit { Color32::from_rgb(7, 16, 31) } else { theme::FG() },
    );
    response.on_hover_text(tip)
}

pub(super) fn chevron_button(ui: &mut Ui, rect: Rect) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new("mini_chevron"), Sense::click());
    if response.hovered() {
        ui.painter()
            .circle_filled(rect.center(), rect.width() / 2.0, theme::PANEL_BG_ALT());
    }
    let c = rect.center();
    ui.painter().add(Shape::line(
        vec![c + vec2(-6.0, 3.0), c + vec2(0.0, -3.0), c + vec2(6.0, 3.0)],
        Stroke::new(1.8, theme::FG_SOFT()),
    ));
    response.on_hover_text("Open the command bar")
}
