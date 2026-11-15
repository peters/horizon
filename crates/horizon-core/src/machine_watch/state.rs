//! The state of the agents of one machine, from the same rules that a local panel
//! reads, so one agent looks the same on a machine and on this computer.
//!
//! The state of one agent has two sources, in one order. The title rule of its kind
//! answers first: a title of the Codex forms says that the program works or that an
//! approval waits, and `tmux` keeps the title of every pane. When the title names no
//! state, the working line of the last rows of the pane answers, as it does for a
//! panel of this computer.
//!
//! The four words follow the attention state of a panel: **Working**, **Needs you**,
//! **Done** for a turn that ended while the person did not look, and no word for an
//! idle agent that the person saw. [`MachineModel::mark_seen`] is the look, and the
//! sidebar of the next round calls it when the person opens the agent or passes it.
//!
//! A pane that the watch sees for the first time claims nothing while it is idle:
//! the watch of a machine may start at any moment, so an idle pane was idle before
//! the watch, and no turn of the person ended there. A pane that is already working
//! when the watch first sees it is **Working**, because the watch may start in the
//! middle of its turn. That is the one difference to the start-up of a local
//! launch: Horizon starts a local program itself and knows that its first frames
//! belong to its load, while a watch finds a program that another person started at
//! another time.

use std::collections::BTreeMap;

use crate::PanelKind;
use crate::agent_work::TitleState;

use super::agents::AgentObservation;

/// The state of one agent of a machine, in the order of weight that a row of the
/// sidebar compares its agents by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentState {
    /// Nothing runs, and the person saw the last turn end.
    #[default]
    Idle,
    /// A turn runs.
    Working,
    /// A turn ended while the person did not look.
    Done,
    /// The title of the agent waits for an answer.
    NeedsYou,
}

impl AgentState {
    /// The one word of a row; an idle agent says nothing.
    #[must_use]
    pub const fn word(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Working => Some("Working"),
            Self::Done => Some("Done"),
            Self::NeedsYou => Some("Needs you"),
        }
    }

    /// The weight of a state: the heaviest state of a machine is the state of its
    /// row.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working => 1,
            Self::Done => 2,
            Self::NeedsYou => 3,
        }
    }

    /// Whether this state waits for the person.
    #[must_use]
    pub const fn waits(self) -> bool {
        matches!(self, Self::Done | Self::NeedsYou)
    }
}

/// One agent of a machine: the pane that runs it, its kind, the name that the
/// person reads and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineAgent {
    /// The name of the machine in the machine list.
    pub machine: String,
    /// The `tmux` session of the pane.
    pub session: String,
    /// The pane id of `tmux`.
    pub pane: String,
    /// The agent kind of the pane.
    pub kind: PanelKind,
    /// The name of the agent: the last part of the folder of its pane.
    pub name: String,
    /// The state of the agent.
    pub state: AgentState,
}

/// The panes of one machine that the watch keeps, and the state of each.
///
/// The identity of a row is the pair of its session and its pane, which `tmux`
/// keeps for the life of the pane. A pane that the next report does not name leaves
/// the model, so a closed session and a closed pane drop their rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineModel {
    machine: String,
    panes: BTreeMap<(String, String), PaneState>,
}

/// One pane of a machine, with what the person saw of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneState {
    kind: PanelKind,
    name: String,
    state: AgentState,
    /// The last report of the pane said that it works.
    working: bool,
    /// A turn ended after the last look of the person, so its row waits for one.
    unread: bool,
}

impl MachineModel {
    /// The model of the machine `machine`, with no pane.
    #[must_use]
    pub fn new(machine: impl Into<String>) -> Self {
        Self {
            machine: machine.into(),
            panes: BTreeMap::new(),
        }
    }

    /// The name of the machine of this model.
    #[must_use]
    pub fn machine(&self) -> &str {
        &self.machine
    }

    /// The rows of the model, in session and pane order.
    #[must_use]
    pub fn rows(&self) -> Vec<MachineAgent> {
        self.panes
            .iter()
            .map(|((session, pane), held)| MachineAgent {
                machine: self.machine.clone(),
                session: session.clone(),
                pane: pane.clone(),
                kind: held.kind,
                name: held.name.clone(),
                state: held.state,
            })
            .collect()
    }

    /// Applies one report of the machine, and answers whether any row changed.
    ///
    /// A pane that the report does not name leaves the model.
    pub fn observe(&mut self, observations: &[AgentObservation]) -> bool {
        let mut seen: Vec<(String, String)> = Vec::with_capacity(observations.len());
        let mut changed = false;
        for observation in observations {
            let key = (observation.session.clone(), observation.pane.clone());
            seen.push(key.clone());
            let working = !matches!(observation.title, TitleState::Plain) || observation.working;
            let state = match observation.title {
                TitleState::NeedsYou => AgentState::NeedsYou,
                TitleState::Working => AgentState::Working,
                TitleState::Plain if observation.working => AgentState::Working,
                TitleState::Plain => AgentState::Idle,
            };
            match self.panes.get_mut(&key) {
                Some(held) => {
                    if held.working && !working {
                        // The turn ended in this report, which is after every look
                        // of the person before it.
                        held.unread = true;
                    }
                    held.working = working;
                    let state = match state {
                        AgentState::Idle if held.unread => AgentState::Done,
                        state => state,
                    };
                    if state != held.state {
                        held.state = state;
                        changed = true;
                    }
                    if held.kind != observation.kind {
                        held.kind = observation.kind;
                        changed = true;
                    }
                    if held.name != observation.name {
                        held.name.clone_from(&observation.name);
                        changed = true;
                    }
                }
                None => {
                    // The first sight of an idle pane claims nothing: the watch may
                    // start long after the turn of another person ended there.
                    self.panes.insert(
                        key,
                        PaneState {
                            kind: observation.kind,
                            name: observation.name.clone(),
                            state,
                            working,
                            unread: false,
                        },
                    );
                    changed = true;
                }
            }
        }
        let before = self.panes.len();
        self.panes.retain(|key, _| seen.contains(key));
        changed || self.panes.len() != before
    }

    /// Records that the person looked at the agent of `session` and `pane`: a
    /// finished turn is no longer unread, and a working agent that ends its turn
    /// later is unread again. Answers whether any row changed.
    pub fn mark_seen(&mut self, session: &str, pane: &str) -> bool {
        let Some(held) = self.panes.get_mut(&(session.to_owned(), pane.to_owned())) else {
            return false;
        };
        let mut changed = held.unread;
        held.unread = false;
        if held.state == AgentState::Done {
            held.state = AgentState::Idle;
            changed = true;
        }
        changed
    }

    /// Drops every pane of the model, so the rows of a stopped watch leave the
    /// snapshot.
    pub fn clear(&mut self) -> bool {
        let changed = !self.panes.is_empty();
        self.panes.clear();
        changed
    }
}
