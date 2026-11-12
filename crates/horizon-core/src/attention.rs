//! Which agent panel needs the person, which one finished while the person was
//! elsewhere, and which one works.
//!
//! One panel has one state. An agent kind whose launch registered a work ledger
//! takes the state from that ledger: the hooks know that a question waits and
//! that a turn ended. Every other agent kind keeps the working indicator of its
//! terminal screen, and a turn that ends while the panel did not have the
//! focus is unread.
use std::collections::BTreeMap;

use crate::PanelId;
use crate::agent_work::{TurnLedger, TurnState};

/// The state of one agent panel, in the order of weight that a workspace row
/// compares its panels by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Attention {
    /// Nothing runs and the person saw the end of the last turn.
    #[default]
    Idle,
    /// A turn is open.
    Working,
    /// The turn ended and the agent still has work in the background.
    Background,
    /// The turn ended and the person has not looked at the panel since.
    Done,
    /// The agent waits for an answer, or its turn failed.
    NeedsYou,
}

impl Attention {
    /// The weight of a state. A workspace row shows the heaviest state of its
    /// panels, so background work weighs as much as a turn that still runs.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working | Self::Background => 1,
            Self::Done => 2,
            Self::NeedsYou => 3,
        }
    }

    /// The one word of a panel row; an idle panel says nothing.
    #[must_use]
    pub const fn word(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Working => Some("Working"),
            Self::Background => Some("Works in the background"),
            Self::Done => Some("Done"),
            Self::NeedsYou => Some("Needs you"),
        }
    }

    /// The sentence of the hover text of a row.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Idle => "Nothing waits for you here",
            Self::Working => "The agent is working",
            Self::Background => "The turn ended and the agent still works in the background",
            Self::Done => "The agent finished while you looked elsewhere",
            Self::NeedsYou => "The agent asked something or its turn failed",
        }
    }

    /// Whether a workspace row shows a badge for it.
    #[must_use]
    pub const fn shows_badge(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Whether the state asks the person to go to the panel.
    #[must_use]
    pub const fn waits(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Done)
    }

    /// The heaviest state of `states`, or idle when there is none.
    #[must_use]
    pub fn heaviest(states: impl IntoIterator<Item = Self>) -> Self {
        states.into_iter().fold(Self::Idle, |heaviest, state| {
            if state.rank() > heaviest.rank() {
                state
            } else {
                heaviest
            }
        })
    }
}

/// The panels of one board and the state of each agent panel.
///
/// The board is told about a ledger read, a working indicator and the person's
/// look. It holds the answers, so a frame that draws a row reads one value and
/// never a file.
#[derive(Clone, Debug, Default)]
pub struct AttentionBoard {
    panels: BTreeMap<String, Panel>,
}

#[derive(Clone, Debug)]
struct Panel {
    /// The session whose events the state follows.
    session_id: String,
    /// The launch whose ledger the state follows. A relaunch of the panel is a
    /// new turn timeline, and its record is read from its own start.
    generation: u64,
    /// The latest moment of a ledger observation, so an older read is dropped.
    observed_at_millis: i64,
    /// Whether the launch of this panel registered a work ledger.
    hooked: bool,
    /// The last state of the ledger.
    state: TurnState,
    /// When the turn reached that state.
    ended_at_millis: Option<i64>,
    /// The background work that the agent reported when its turn ended.
    background_tasks: u32,
    /// The working indicator of a panel without lifecycle hooks.
    working: bool,
    /// The last moment the person looked at the panel.
    seen_at_millis: i64,
    /// The person marked the panel unread, which no later look undoes until
    /// the panel is looked at again.
    marked_unread: bool,
}

impl Panel {
    fn new(now_millis: i64) -> Self {
        Self {
            session_id: String::new(),
            generation: 0,
            observed_at_millis: i64::MIN,
            hooked: false,
            state: TurnState::Unknown,
            ended_at_millis: None,
            background_tasks: 0,
            working: false,
            seen_at_millis: now_millis,
            marked_unread: false,
        }
    }

    fn unseen(&self) -> bool {
        self.marked_unread || self.ended_at_millis.is_some_and(|ended| self.seen_at_millis < ended)
    }

    fn attention(&self) -> Attention {
        // A panel without hooks has no ledger: its working indicator is the
        // signal, and the state that the indicator left is unread until a look.
        if !self.hooked && self.working {
            return Attention::Working;
        }
        let mut state = match self.state {
            // An open question is the person's answer to give, so a look does
            // not clear it. The ledger clears it when the answer arrives.
            TurnState::Blocked => Attention::NeedsYou,
            TurnState::Failed if self.unseen() => Attention::NeedsYou,
            TurnState::Working => Attention::Working,
            TurnState::Finished if self.background_tasks > 0 => Attention::Background,
            TurnState::Finished | TurnState::Interrupted if self.unseen() => Attention::Done,
            _ => Attention::Idle,
        };
        if self.marked_unread && state == Attention::Idle {
            state = Attention::Done;
        }
        state
    }

    fn apply(&mut self, state: TurnState, ended_at_millis: Option<i64>, background_tasks: u32) {
        self.state = state;
        self.ended_at_millis = ended_at_millis;
        self.background_tasks = background_tasks;
    }
}

impl AttentionBoard {
    /// Records one read of the work ledger of `panel`, whose launch registered
    /// that ledger. Answers whether the state of the panel changed.
    pub fn observe_ledger(&mut self, panel: &str, ledger: &TurnLedger, now_millis: i64) -> bool {
        let before = self.state(panel);
        let held = self
            .panels
            .entry(panel.to_owned())
            .or_insert_with(|| Panel::new(now_millis));
        // A record of another session or of another launch of the panel starts
        // a timeline of its own. A read of the same launch that is older than
        // the one this panel holds describes a turn that is already behind it.
        let first = held.observed_at_millis == i64::MIN;
        let relaunched = held.hooked && (ledger.session_id != held.session_id || ledger.generation != held.generation);
        if first || relaunched || ledger.updated_at_millis >= held.observed_at_millis {
            if first || relaunched {
                // A panel that the board reads for the first time, or after a
                // relaunch, is not asked about a turn that ended before that.
                held.seen_at_millis = ledger.updated_at_millis;
            }
            held.session_id.clone_from(&ledger.session_id);
            held.generation = ledger.generation;
            held.observed_at_millis = ledger.updated_at_millis;
            held.hooked = true;
            let (state, ended_at) = ledger_outcome(ledger);
            held.apply(state, ended_at, ledger.background_tasks);
        }
        let after = self.state(panel);
        after != before
    }

    /// Records the working indicator of a panel whose agent kind has no
    /// lifecycle hooks. A panel that has a ledger keeps the ledger's answer.
    pub fn observe_working(&mut self, panel: &str, working: bool, focused: bool, now_millis: i64) -> bool {
        let before = self.state(panel);
        let held = self
            .panels
            .entry(panel.to_owned())
            .or_insert_with(|| Panel::new(now_millis));
        if !held.hooked {
            let was_working = held.working;
            held.working = working;
            if was_working && !working && !focused {
                held.ended_at_millis = Some(now_millis);
                held.state = TurnState::Finished;
            }
        }
        self.state(panel) != before
    }

    /// The person looked at the panel: they gave it the keyboard focus or
    /// clicked its row. An open question stays open, because only the ledger
    /// knows that it was answered.
    pub fn note_look(&mut self, panel: &str, now_millis: i64) -> bool {
        let before = self.state(panel);
        if let Some(held) = self.panels.get_mut(panel) {
            held.seen_at_millis = held.seen_at_millis.max(now_millis);
            held.marked_unread = false;
        }
        self.state(panel) != before
    }

    /// The person marked an idle agent panel unread. Answers whether the panel
    /// shows another state now.
    pub fn mark_unread(&mut self, panel: &str) -> bool {
        let before = self.state(panel);
        let now_millis = self.panels.get(panel).map_or(0, |held| held.seen_at_millis);
        let held = self
            .panels
            .entry(panel.to_owned())
            .or_insert_with(|| Panel::new(now_millis));
        held.marked_unread = true;
        self.state(panel) != before
    }

    /// The panel left the board.
    pub fn forget(&mut self, panel: &str) {
        self.panels.remove(panel);
    }

    /// Drops every panel that `keep` refuses, so a board that closed a panel
    /// keeps no state of it.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.panels.retain(|panel, _| keep(panel));
    }

    /// When the turn of `panel` reached the state that a row shows, so the
    /// panel that waited longest comes first.
    #[must_use]
    pub fn since_millis(&self, panel: &str) -> i64 {
        let Some(held) = self.panels.get(panel) else {
            return 0;
        };
        held.ended_at_millis.unwrap_or_else(|| held.observed_at_millis.max(0))
    }

    /// The state of `panel`; an agent panel that the board knows nothing about
    /// is idle.
    #[must_use]
    pub fn state(&self, panel: &str) -> Attention {
        self.panels.get(panel).map_or(Attention::Idle, Panel::attention)
    }

    /// Whether the launch of `panel` registered a work ledger, so its state
    /// comes from the lifecycle hooks and not from its terminal screen.
    #[must_use]
    pub fn from_ledger(&self, panel: &str) -> bool {
        self.panels.get(panel).is_some_and(|held| held.hooked)
    }
}

/// The turn state that a ledger leads to, and the moment the turn ended when
/// the ledger holds a turn that ended.
fn ledger_outcome(ledger: &TurnLedger) -> (TurnState, Option<i64>) {
    let ended_at = match ledger.state {
        TurnState::Finished | TurnState::Interrupted | TurnState::Failed => Some(ledger.updated_at_millis),
        TurnState::Unknown | TurnState::Working | TurnState::Blocked => None,
    };
    (ledger.state, ended_at)
}

/// One agent panel that the person can go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttentionTarget {
    pub panel: PanelId,
    pub attention: Attention,
    /// When the turn reached that state, oldest first.
    pub since_millis: i64,
}

/// The panel that the next jump goes to: the panels that need the person first,
/// the oldest one of them, then the panels that finished unseen, oldest first.
/// A press goes on to the panel after the one that has the focus, so each press
/// reaches the next agent and the last one comes round to the first.
#[must_use]
pub fn next_attention_target(targets: &[AttentionTarget], focused: Option<PanelId>) -> Option<PanelId> {
    let mut waiting: Vec<&AttentionTarget> = targets.iter().filter(|target| target.attention.waits()).collect();
    waiting.sort_by_key(|target| {
        (
            std::cmp::Reverse(target.attention.rank()),
            target.since_millis,
            target.panel.0,
        )
    });
    let after = focused
        .and_then(|panel| waiting.iter().position(|target| target.panel == panel))
        .map_or(0, |position| (position + 1) % waiting.len().max(1));
    waiting
        .get(after)
        .or_else(|| waiting.first())
        .map(|target| target.panel)
}

#[cfg(test)]
mod tests;
