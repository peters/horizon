//! The agents of a machine under its row, in the form of the panel rows of a workspace.
//! The rows come from the snapshot of the watch, and a machine whose link is down, or
//! whose card link is not up, says in one quiet line what happened to them.

use egui::{Rect, Sense, Vec2, pos2};
use horizon_core::machine_watch::{AgentState, MachineAgent, MachineWatchState, WatchLink};
use horizon_core::session_pickup::relative_time;
use horizon_core::workspace_list::Waiting;

use crate::theme;

use super::super::panel_rows::{self, PANEL_ROW_HEIGHT, PanelRowLook, PanelRowState};
use super::super::{SidebarActions, text};
use super::{MachineRow, card, dot_center};

/// The sentence of a machine that runs no `tmux`.
pub(super) const NO_TMUX: &str = "No tmux on this machine.";

const QUIET_HEIGHT: f32 = 24.0;
/// The line of a machine whose card link is not up: its `ssh` child is gone.
pub(super) const CARDS_DOWN: &str = "Cards: no link · trying again";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct MachineWatchRow {
    /// `link lost 2 minutes ago`, in the place of the provider line.
    pub(in crate::app) lost: Option<String>,
    pub(in crate::app) quiet: Option<Quiet>,
    /// The agents of the machine, in session and pane order.
    pub(in crate::app) agents: Vec<MachineAgent>,
    /// The summary of the folded row: `1 needs you · 1 done`, `2 working`, or nothing.
    pub(in crate::app) summary: Option<String>,
}

/// The one line in the place of the agent rows of a machine: the words that fit the
/// sidebar, and the whole sentence for the hover and for a screen reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Quiet {
    /// The words that the row draws.
    line: String,
    /// The whole sentence.
    sentence: String,
}

impl Quiet {
    /// One line of `line`, with the whole sentence that a reader hears.
    fn of(line: &str, sentence: &str) -> Self {
        Self {
            line: line.to_owned(),
            sentence: sentence.to_owned(),
        }
    }
}

impl MachineWatchRow {
    pub(in crate::app) fn of(state: Option<&MachineWatchState>, now_millis: i64, cards: Option<&str>) -> Self {
        let cards = cards.map(|sentence| Quiet::of(CARDS_DOWN, sentence));
        let Some(state) = state else {
            return Self {
                quiet: cards,
                ..Self::default()
            };
        };
        let (lost, quiet) = match state.link {
            WatchLink::Lost {
                since_millis, agents, ..
            } => (
                Some(format!(
                    "link lost {}",
                    relative_time(now_millis, i64::try_from(since_millis).unwrap_or(i64::MAX))
                )),
                Some(last_seen(agents)),
            ),
            WatchLink::NoTmux => (None, Some(Quiet::of(NO_TMUX, NO_TMUX))),
            WatchLink::Refused { reason, .. } => (None, Some(Quiet::of(reason.sentence(), reason.sentence()))),
            // A link that connects or that is up says nothing in these two lines, and so
            // does the watch of a machine that is off.
            _ => (None, None),
        };
        Self {
            lost,
            quiet: quiet.filter(|_| state.agents.is_empty()).or(cards),
            summary: summary_of(&state.agents),
            agents: state.agents.clone(),
        }
    }

    /// Paints the agent rows of the machine under its row, or its one quiet line.
    pub(super) fn paint_rows(&self, ui: &mut egui::Ui, row: &MachineRow, actions: &mut SidebarActions) {
        let last = self.agents.len().saturating_sub(1);
        for (index, agent) in self.agents.iter().enumerate() {
            paint_agent(ui, row, agent, index == last, actions);
        }
        if let Some(line) = &self.quiet {
            paint_quiet_line(ui, line);
        }
    }
}

/// Paints one agent of the machine as a panel row, and turns a press on it into an action.
fn paint_agent(ui: &mut egui::Ui, row: &MachineRow, agent: &MachineAgent, last: bool, actions: &mut SidebarActions) {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), PANEL_ROW_HEIGHT)).1;
    let hovered = ui.rect_contains_pointer(rect);
    let look = PanelRowLook {
        guide_x: dot_center(card(rect)).x,
        accent: theme::ACCENT(),
        kind: agent.kind,
        focused: false,
        label: &agent.name,
        attention: agent.state.attention(),
    };
    panel_rows::paint_panel_row(
        ui,
        rect,
        &look,
        PanelRowState {
            hovered,
            close: None,
            last,
        },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("sidebar_machine_agent", row.name.as_str(), &agent.session, &agent.pane)),
        Sense::click(),
    );
    let spoken = text::panel_spoken(agent.kind, &agent.name, agent.state.attention());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
    let hint = if agent.can_open() {
        spoken
    } else {
        format!("{spoken}\n{CANNOT_OPEN}")
    };
    if response.on_hover_text(hint).clicked() && agent.can_open() {
        actions.open_machine_agent = Some(agent.clone());
    }
}

/// What a row says about an agent whose session name no terminal of this build opens.
pub(super) const CANNOT_OPEN: &str = "The name of this tmux session is not one word, so Horizon does not open it.";

/// The one line in the place of the agent rows of a machine whose link is down: the
/// words fit the sidebar, and the whole sentence is in the hover.
fn last_seen(agents: usize) -> Quiet {
    let word = if agents == 1 { "agent" } else { "agents" };
    Quiet {
        line: format!("{agents} {word} · trying again"),
        sentence: format!("Last seen: {agents} {word}. Horizon tries again."),
    }
}

/// The agents that wait for the person in the words of a workspace row, then the ones
/// that work while none waits.
fn summary_of(agents: &[MachineAgent]) -> Option<String> {
    let count = |state: AgentState| agents.iter().filter(|agent| agent.state == state).count();
    let waiting = Waiting {
        needs_you: count(AgentState::NeedsYou),
        done: count(AgentState::Done),
    };
    waiting.line().or_else(|| match count(AgentState::Working) {
        0 => None,
        working => Some(format!("{working} working")),
    })
}

/// Paints the quiet line of a machine in the colour and size of a row's second line. Its
/// whole sentence is in its hover and in its spoken text.
fn paint_quiet_line(ui: &mut egui::Ui, quiet: &Quiet) {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), QUIET_HEIGHT)).1;
    let card = card(rect);
    let left = dot_center(card).x + 13.0;
    let galley = text::line(
        ui,
        &quiet.line,
        11.0,
        text::REGULAR,
        text::secondary(),
        card.max.x - 8.0 - left,
    );
    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        text::secondary(),
    );
    let response = ui.interact(
        Rect::from_min_max(pos2(left, rect.min.y), pos2(card.max.x, rect.max.y)),
        ui.make_persistent_id(("sidebar_machine_quiet", &quiet.line)),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &quiet.sentence));
    response.on_hover_text(&quiet.sentence);
}

/// What the fold control says to a screen reader.
pub(super) fn fold_spoken(folded: bool, machine: &str) -> String {
    if folded {
        format!("Show the agents of {machine}")
    } else {
        format!("Fold the agents of {machine}")
    }
}
