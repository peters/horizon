//! The agents of a machine under its row, in the form of the panel rows of a workspace:
//! the tree guide from the state dot of the machine, the kind label, the name of the
//! agent and, at the right, the word and the dot of the same state that a panel row
//! shows.
//!
//! The rows come from the snapshot of the watch, which a frame reads once and rebuilds
//! only when its version changed. A machine whose link is down has no rows and says in
//! one quiet line what happened to them.

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

/// The height of the quiet line in the place of the agent rows.
const QUIET_HEIGHT: f32 = 24.0;

/// What the watch of one machine says, in the words of its row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct MachineWatchRow {
    /// `link lost 2 minutes ago`, in the place of the provider line.
    pub(in crate::app) lost: Option<String>,
    /// The one line in the place of the agent rows.
    pub(in crate::app) quiet: Option<String>,
    /// The agents of the machine, in session and pane order.
    pub(in crate::app) agents: Vec<MachineAgent>,
    /// The summary of the folded row: `1 needs you · 1 done`, `2 working`, or nothing.
    pub(in crate::app) summary: Option<String>,
}

impl MachineWatchRow {
    /// The watch of one machine of the snapshot, in the words of a row.
    pub(in crate::app) fn of(state: Option<&MachineWatchState>, now_millis: i64) -> Self {
        let Some(state) = state else {
            return Self::default();
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
            WatchLink::NoTmux => (None, Some(NO_TMUX.to_owned())),
            WatchLink::Refused { reason, .. } => (None, Some(reason.sentence().to_owned())),
            WatchLink::Connecting
            | WatchLink::Watching
            | WatchLink::NoServer
            | WatchLink::OverTheLimit
            | WatchLink::Off => (None, None),
        };
        Self {
            lost,
            quiet,
            summary: summary_of(&state.agents),
            agents: state.agents.clone(),
        }
    }

    /// Paints the agent rows of the machine under its row, or the one quiet line in
    /// their place. A machine with no agent paints nothing.
    pub(super) fn paint_rows(&self, ui: &mut egui::Ui, row: &MachineRow, actions: &mut SidebarActions) {
        if !self.agents.is_empty() {
            let last = self.agents.len() - 1;
            for (index, agent) in self.agents.iter().enumerate() {
                paint_agent(ui, row, agent, index == last, actions);
            }
            return;
        }
        if let Some(line) = &self.quiet {
            paint_quiet_line(ui, line);
        }
    }
}

/// Paints one agent of the machine, as a panel row of a workspace is painted, and turns
/// a press on it into an action.
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
    let response = if agent.can_open() {
        response.on_hover_text(spoken)
    } else {
        response.on_hover_text(format!("{spoken}\n{}", cannot_open()))
    };
    if response.clicked() {
        actions.open_machine_agent = Some(agent.clone());
    }
}

/// What a row says about an agent whose session name no terminal of this build can
/// open.
pub(super) const fn cannot_open() -> &'static str {
    "The name of this tmux session is not one word, so Horizon does not open it."
}

/// The one line in the place of the agent rows of a machine whose link is down.
fn last_seen(agents: usize) -> String {
    format!(
        "Last seen: {agents} {}. Horizon tries again.",
        if agents == 1 { "agent" } else { "agents" }
    )
}

/// The summary of the folded row: the agents that wait for the person in the words of
/// a workspace row, then the agents that work while none waits.
fn summary_of(agents: &[MachineAgent]) -> Option<String> {
    let waiting = agents.iter().fold(Waiting::default(), |mut waiting, agent| {
        match agent.state {
            AgentState::NeedsYou => waiting.needs_you += 1,
            AgentState::Done => waiting.done += 1,
            AgentState::Idle | AgentState::Working => {}
        }
        waiting
    });
    if let Some(line) = waiting.line() {
        return Some(line);
    }
    let working = agents.iter().filter(|agent| agent.state == AgentState::Working).count();
    (working > 0).then(|| format!("{working} working"))
}

/// Paints the one quiet line in the place of the agent rows, in the colour and the
/// size of the second line of a row. A line that is too long for the sidebar is cut,
/// and its whole text is in its hover.
fn paint_quiet_line(ui: &mut egui::Ui, line: &str) {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), QUIET_HEIGHT)).1;
    let card = card(rect);
    let left = dot_center(card).x + 13.0;
    let galley = text::line(
        ui,
        line,
        11.0,
        text::REGULAR,
        text::secondary(),
        card.max.x - 8.0 - left,
    );
    let elided = galley.elided;
    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        text::secondary(),
    );
    let response = ui.interact(
        Rect::from_min_max(pos2(left, rect.min.y), pos2(card.max.x, rect.max.y)),
        ui.make_persistent_id(("sidebar_machine_quiet", line)),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, line));
    if elided {
        response.on_hover_text(line);
    }
}

/// The mark of the fold control of a machine: folded, or open.
pub(super) const fn fold_mark(folded: bool) -> &'static str {
    if folded { "\u{25B8}" } else { "\u{25BE}" }
}

/// What the fold control says to a screen reader.
pub(super) fn fold_spoken(folded: bool, machine: &str) -> String {
    if folded {
        format!("Show the agents of {machine}")
    } else {
        format!("Fold the agents of {machine}")
    }
}
