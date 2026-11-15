//! The state of the agents of one machine, from the same rules that a local panel
//! reads, so one agent looks the same on a machine and on this computer.
//!
//! The state that the program of a pane reports in the pane option `@horizon_state`
//! answers first, because that program knows its own state. Without a report, the title
//! rule of the kind answers, because `tmux` keeps the title of every pane; when the title
//! names no state either, the working line of the last rows of the screen answers. The
//! four words follow the attention state of a panel, and [`MachineModel::mark_seen`] is
//! the look of the person.
//!
//! A pane that the watch sees for the first time claims nothing while it is idle, so no
//! turn ended there. A pane that is already working is **Working**, because the watch may
//! start in the middle of its turn: that is the one difference to a local launch.

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
    /// The state that one word of the pane option `@horizon_state` names, or `None` for
    /// a word outside the four of the contract: the text of a machine is untrusted, so
    /// another word of it is no state.
    #[must_use]
    pub fn claim_of(word: &str) -> Option<Self> {
        match word {
            "working" => Some(Self::Working),
            "blocked" => Some(Self::NeedsYou),
            "done" => Some(Self::Done),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }

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

    /// The state of a panel that this agent is: the word, the colour and the dot of
    /// a row of a machine come from the type of a local panel, so one agent looks the
    /// same on a machine and on this computer.
    #[must_use]
    pub const fn attention(self) -> crate::Attention {
        match self {
            Self::Idle => crate::Attention::Idle,
            Self::Working => crate::Attention::Working,
            Self::Done => crate::Attention::Done,
            Self::NeedsYou => crate::Attention::NeedsYou,
        }
    }
}

/// One agent of a machine: the pane that runs it, its kind, the name that the person
/// reads and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineAgent {
    /// The name of the machine in the machine list.
    pub machine: String,
    /// The `tmux` session of the pane, and its id in `tmux`.
    pub session: String,
    pub pane: String,
    pub kind: PanelKind,
    /// The name of the agent: the last part of the folder of its pane.
    pub name: String,
    pub state: AgentState,
}

/// The panes of one machine that the watch keeps, and the state of each. The identity
/// of a row is the pair of its session and its pane; a pane that the next report does
/// not name leaves the model.
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
    /// The last report said that the pane works.
    working: bool,
    /// A turn ended after the last look of the person.
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

    /// How many agents this machine reported in its last report.
    #[must_use]
    pub fn len(&self) -> usize {
        self.panes.len()
    }

    /// Whether this machine reported no agent.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// Applies one report of the machine, and answers whether any row changed. A
    /// pane that the report does not name leaves the model.
    pub fn observe(&mut self, observations: &[AgentObservation]) -> bool {
        let mut seen: Vec<(String, String)> = Vec::with_capacity(observations.len());
        let mut changed = false;
        for observation in observations {
            let key = (observation.session.clone(), observation.pane.clone());
            seen.push(key.clone());
            let held = self.panes.get(&key);
            // The report of the program of the pane answers first, then the title of its
            // program, then the working line of its screen. A report that waits or that
            // works is a turn that runs, as a title of those two states is.
            let claim = observation.claim;
            let working = match claim {
                Some(AgentState::Working | AgentState::NeedsYou) => true,
                Some(AgentState::Idle | AgentState::Done) => false,
                None => !matches!(observation.title, TitleState::Plain) || observation.working,
            };
            // A turn that ends in this report ends after every look before it.
            let unread = held.is_some_and(|held| held.unread || (held.working && !working));
            let state = match claim {
                Some(AgentState::Done) if !unread => AgentState::Idle,
                Some(state) => state,
                None => match observation.title {
                    TitleState::NeedsYou => AgentState::NeedsYou,
                    TitleState::Working => AgentState::Working,
                    TitleState::Plain if working => AgentState::Working,
                    TitleState::Plain if unread => AgentState::Done,
                    TitleState::Plain => AgentState::Idle,
                },
            };
            let next = PaneState {
                kind: observation.kind,
                name: observation.name.clone(),
                state,
                working,
                unread,
            };
            changed |= held.is_none_or(|held| held.state != state || held.kind != next.kind || held.name != next.name);
            self.panes.insert(key, next);
        }
        let before = self.panes.len();
        self.panes.retain(|key, _| seen.contains(key));
        changed || self.panes.len() != before
    }

    /// Records that the person looked at the agent of `session` and `pane`: a
    /// finished turn is no longer unread, and a working agent that ends its turn
    /// later is unread again.
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

    /// Drops every pane, so the rows of a stopped watch leave the snapshot.
    pub fn clear(&mut self) -> bool {
        let changed = !self.panes.is_empty();
        self.panes.clear();
        changed
    }
}
