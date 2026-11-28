//! The workspace list: the attention badge of each workspace of the sidebar and
//! the one line of facts under its name. The UI reads the same rows.
//!
//! The line says what Horizon knows about the workspace: the state of its
//! orchestrator, the agents that wait for the person and the machine that it runs
//! on. It holds no text of the screen of a panel: such a line is what a program
//! printed, with its prompts and its control characters, and it is not a statement
//! about the workspace.
use crate::attention::Attention;

/// How many agent panels of a workspace wait for the person.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    /// The panels that ask a question or failed a turn.
    pub needs_you: usize,
    /// The panels that finished while the person looked elsewhere.
    pub done: usize,
}

impl Waiting {
    /// The count at the front of the line of a row, or `None` when no panel waits.
    #[must_use]
    pub fn line(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.needs_you > 0 {
            parts.push(format!(
                "{} {}",
                self.needs_you,
                if self.needs_you == 1 { "needs you" } else { "need you" }
            ));
        }
        if self.done > 0 {
            parts.push(format!("{} done", self.done));
        }
        (!parts.is_empty()).then(|| parts.join(" \u{00B7} "))
    }
}

/// What the dock knows about the turns of the orchestrator of one workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// A turn of the program runs now.
    pub busy: bool,
    /// How many turns of the conversation ended with a failure, as the
    /// conversation names each of them in one line. A reader that knows the
    /// count of an earlier moment tells a turn that failed from one that ended
    /// well.
    pub failed: usize,
}

/// The end of one turn of the orchestrator of a workspace that the person did
/// not see in the dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnEnd {
    /// The turn ended without a failure.
    Done,
    /// The turn failed.
    Failed,
}

/// The state of the orchestrator of a workspace: the one word at the front of
/// the line of its row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orchestrator {
    /// No question waits, no turn runs and no turn ended unseen, so the row says
    /// no word.
    #[default]
    Quiet,
    /// A card of the workspace asks a question that nobody answered.
    NeedsYou,
    /// The last turn of the orchestrator ended with a failure that the person
    /// did not see.
    Failed,
    /// A turn of the orchestrator runs now.
    Working,
    /// The last turn of the orchestrator ended well and the person did not see
    /// it in the dock.
    Done,
}

impl Orchestrator {
    /// The state of a workspace, in the order of weight of one word: a question,
    /// then a failed turn, then a working turn, then a turn that ended unseen.
    #[must_use]
    pub const fn of(questions: usize, turn: Turn, unseen: Option<TurnEnd>) -> Self {
        if questions > 0 {
            return Self::NeedsYou;
        }
        match unseen {
            Some(TurnEnd::Failed) => Self::Failed,
            _ if turn.busy => Self::Working,
            Some(TurnEnd::Done) => Self::Done,
            None => Self::Quiet,
        }
    }

    /// The one word of the line, or `None` for a workspace that says no word.
    #[must_use]
    pub const fn word(self) -> Option<&'static str> {
        match self {
            Self::Quiet => None,
            Self::NeedsYou => Some("Needs you"),
            Self::Failed => Some("Failed"),
            Self::Working => Some("Working"),
            Self::Done => Some("Done"),
        }
    }

    /// The state of this workspace as one machine-readable word, which the read
    /// of the work items answers: `idle` for a workspace that says no word,
    /// `needs_you`, `failed`, `working` or `done` for the state that its row
    /// shows.
    #[must_use]
    pub const fn state(self) -> &'static str {
        match self {
            Self::Quiet => "idle",
            Self::NeedsYou => "needs_you",
            Self::Failed => "failed",
            Self::Working => "working",
            Self::Done => "done",
        }
    }

    /// The state of this workspace in the words of the row of a work item:
    /// `working`, `needs you`, `failed` or `done`, lower case, or `None` for a
    /// workspace that says no word. A program reads [`Self::state`] instead.
    #[must_use]
    pub const fn row_word(self) -> Option<&'static str> {
        match self {
            Self::Quiet => None,
            Self::NeedsYou => Some("needs you"),
            Self::Failed => Some("failed"),
            Self::Working => Some("working"),
            Self::Done => Some("done"),
        }
    }

    /// The words that Horizon writes in the conversation of the front desk when a
    /// work item goes into this state, or `None` for a state that it does not
    /// name.
    #[must_use]
    pub const fn notice_word(self) -> Option<&'static str> {
        match self {
            Self::NeedsYou => Some("needs you"),
            Self::Done => Some("finished"),
            Self::Failed => Some("failed"),
            Self::Quiet | Self::Working => None,
        }
    }

    /// The state that the row carries, from the heaviest state of its panels: a
    /// question and a failed turn take the mark of a panel that needs the
    /// person, a finished turn the mark of a finished panel, and a working turn
    /// the state of the panels themselves.
    #[must_use]
    pub const fn attention(self, panels: Attention) -> Attention {
        match self {
            Self::NeedsYou | Self::Failed => Attention::NeedsYou,
            Self::Done => Attention::Done,
            Self::Working | Self::Quiet => panels,
        }
    }
}

/// One row of the workspace list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The heaviest attention state of the panels of the workspace, or the state
    /// that its orchestrator shows.
    pub attention: Attention,
    /// How many of its agent panels wait for the person.
    pub waiting: Waiting,
    /// The line under the name: the word of the orchestrator, then the count of
    /// the agents that wait and `on <machine>`. It is empty for a workspace of
    /// this computer where no agent waits and no orchestrator speaks.
    pub line: String,
    /// The facts of the line without the word of the orchestrator: the count of
    /// the agents that wait, then `on <machine>`. It is empty for a workspace of
    /// this computer where no agent waits.
    pub facts: String,
    /// The state of the orchestrator of the workspace, which the row shows as the
    /// one word at its right edge.
    pub orchestrator: Orchestrator,
}

impl Row {
    /// The row of a workspace that runs on this computer, where no agent waits.
    #[must_use]
    pub fn local(attention: Attention) -> Self {
        Self::placed(attention, Waiting::default(), None, Orchestrator::Quiet)
    }

    /// The row of a workspace that runs on this computer or on a machine.
    ///
    /// The word of the orchestrator comes first, then the count, so a long
    /// machine name cannot cut either of them off the row.
    #[must_use]
    pub fn placed(attention: Attention, waiting: Waiting, machine: Option<&str>, orchestrator: Orchestrator) -> Self {
        let place = machine.map(|machine| format!("on {machine}"));
        let facts = match (waiting.line(), place) {
            (Some(count), Some(place)) => format!("{count} \u{00B7} {place}"),
            (Some(line), None) | (None, Some(line)) => line,
            (None, None) => String::new(),
        };
        let line = match (orchestrator.word(), facts.is_empty()) {
            (Some(word), false) => format!("{word} \u{00B7} {facts}"),
            (Some(word), true) => word.to_owned(),
            (None, _) => facts.clone(),
        };
        Self {
            attention: orchestrator.attention(attention),
            waiting,
            line,
            facts,
            orchestrator,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_says_where_its_workspace_runs_and_nothing_of_a_screen() {
        let none = Waiting::default();
        assert_eq!(
            Row::placed(Attention::Working, none, Some("build-1"), Orchestrator::Quiet).line,
            "on build-1",
            "a row says where the workspace lives"
        );
        assert_eq!(
            Row::placed(Attention::Working, none, None, Orchestrator::Quiet).line,
            ""
        );
        assert_eq!(Row::local(Attention::Working).line, "");
    }

    #[test]
    fn a_row_that_waits_starts_its_line_with_the_count_of_the_agents() {
        assert_eq!(Waiting { needs_you: 2, done: 0 }.line().as_deref(), Some("2 need you"));
        assert_eq!(
            Waiting { needs_you: 1, done: 0 }.line().as_deref(),
            Some("1 needs you"),
            "one agent needs the person, so its count takes the singular verb"
        );
        assert_eq!(Waiting { needs_you: 0, done: 1 }.line().as_deref(), Some("1 done"));
        assert_eq!(Waiting { needs_you: 0, done: 2 }.line().as_deref(), Some("2 done"));
        assert_eq!(
            Waiting { needs_you: 1, done: 2 }.line().as_deref(),
            Some("1 needs you \u{00B7} 2 done")
        );
        assert_eq!(Waiting::default().line(), None);
        let waiting = Waiting { needs_you: 2, done: 0 };
        assert_eq!(
            Row::placed(Attention::NeedsYou, waiting, None, Orchestrator::Quiet).line,
            "2 need you"
        );
        assert_eq!(
            Row::placed(Attention::Done, waiting, Some("build-1"), Orchestrator::Quiet).line,
            "2 need you \u{00B7} on build-1",
            "a count is never cut off, also when the workspace says where it lives"
        );
    }

    #[test]
    fn the_word_of_an_orchestrator_comes_first_in_the_line_of_its_row() {
        let waiting = Waiting { needs_you: 1, done: 0 };
        let row = Row::placed(Attention::Idle, waiting, Some("build-1"), Orchestrator::Done);
        assert_eq!(row.line, "Done \u{00B7} 1 needs you \u{00B7} on build-1");
        assert_eq!(row.attention, Attention::Done, "a finished turn marks the row");

        // A word alone, and no line of facts to join it to.
        let alone = Row::placed(Attention::Idle, Waiting::default(), None, Orchestrator::Working);
        assert_eq!(alone.line, "Working");
        assert_eq!(alone.attention, Attention::Idle, "no agent of the panel waits");
    }

    #[test]
    fn one_word_says_the_heaviest_state_of_a_workspace() {
        let quiet = Turn { busy: false, failed: 0 };
        assert_eq!(Orchestrator::of(1, quiet, None), Orchestrator::NeedsYou);
        assert_eq!(
            Orchestrator::of(1, Turn { busy: true, failed: 0 }, Some(TurnEnd::Failed)),
            Orchestrator::NeedsYou,
            "a question outranks every turn"
        );
        assert_eq!(
            Orchestrator::of(0, Turn { busy: true, failed: 1 }, Some(TurnEnd::Failed)),
            Orchestrator::Failed,
            "a failed turn outranks a working one"
        );
        assert_eq!(
            Orchestrator::of(0, Turn { busy: true, failed: 1 }, Some(TurnEnd::Done)),
            Orchestrator::Working,
            "a working turn outranks a finished one"
        );
        assert_eq!(Orchestrator::of(0, quiet, Some(TurnEnd::Done)), Orchestrator::Done);
        assert_eq!(Orchestrator::of(0, quiet, None), Orchestrator::Quiet);
        assert_eq!(Orchestrator::Quiet.word(), None);
        assert_eq!(Orchestrator::Done.word(), Some("Done"));
        assert_eq!(Orchestrator::Working.word(), Some("Working"));
        assert_eq!(Orchestrator::Failed.word(), Some("Failed"));
        assert_eq!(Orchestrator::NeedsYou.word(), Some("Needs you"));
    }

    #[test]
    fn the_state_of_a_work_item_keeps_its_machine_word_and_gets_one_word_of_a_row() {
        assert_eq!(
            [
                Orchestrator::Quiet.state(),
                Orchestrator::NeedsYou.state(),
                Orchestrator::Failed.state(),
                Orchestrator::Working.state(),
                Orchestrator::Done.state(),
            ],
            ["idle", "needs_you", "failed", "working", "done"],
            "the answer of the work items keeps the machine words"
        );
        assert_eq!(
            [
                Orchestrator::Quiet.row_word(),
                Orchestrator::NeedsYou.row_word(),
                Orchestrator::Failed.row_word(),
                Orchestrator::Working.row_word(),
                Orchestrator::Done.row_word(),
            ],
            [None, Some("needs you"), Some("failed"), Some("working"), Some("done")],
            "the row of a work item says no word for a workspace that says none, and lower case otherwise"
        );
    }

    #[test]
    fn the_mark_of_a_workspace_follows_the_word_and_the_panels_behind_it() {
        assert_eq!(Orchestrator::NeedsYou.attention(Attention::Idle), Attention::NeedsYou);
        assert_eq!(
            Orchestrator::Failed.attention(Attention::Idle),
            Attention::NeedsYou,
            "a failed turn asks the person, as an agent panel that failed does"
        );
        assert_eq!(Orchestrator::Done.attention(Attention::Idle), Attention::Done);
        assert_eq!(
            Orchestrator::Working.attention(Attention::Working),
            Attention::Working,
            "a working turn keeps the state of the attention model"
        );
        assert_eq!(
            Orchestrator::Working.attention(Attention::Idle),
            Attention::Idle,
            "a row whose agent panels are quiet keeps the state of today"
        );
        assert_eq!(Orchestrator::Quiet.attention(Attention::NeedsYou), Attention::NeedsYou);
    }
}
