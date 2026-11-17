//! The workspace list: the attention badge of each workspace of the sidebar and
//! the one line of facts under its name. The UI reads the same rows.
//!
//! The line says what Horizon knows about the workspace: the agents that wait
//! for the person and the machine that it runs on. It holds no text of the
//! screen of a panel: such a line is what a program printed, with its prompts
//! and its control characters, and it is not a statement about the workspace.
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

/// One row of the workspace list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The heaviest attention state of the panels of the workspace.
    pub attention: Attention,
    /// How many of its agent panels wait for the person.
    pub waiting: Waiting,
    /// The line under the name: the count of the agents that wait, then
    /// `on <machine>`. It is empty for a workspace of this computer where no
    /// agent waits.
    pub line: String,
}

impl Row {
    /// The row of a workspace that runs on this computer, where no agent waits.
    #[must_use]
    pub fn local(attention: Attention) -> Self {
        Self::placed(attention, Waiting::default(), None)
    }

    /// The row of a workspace that runs on this computer or on a machine.
    ///
    /// The count comes first, so a long machine name cannot cut it off the row.
    #[must_use]
    pub fn placed(attention: Attention, waiting: Waiting, machine: Option<&str>) -> Self {
        let place = machine.map(|machine| format!("on {machine}"));
        let line = match (waiting.line(), place) {
            (Some(count), Some(place)) => format!("{count} \u{00B7} {place}"),
            (Some(line), None) | (None, Some(line)) => line,
            (None, None) => String::new(),
        };
        Self {
            attention,
            waiting,
            line,
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
            Row::placed(Attention::Working, none, Some("build-1")).line,
            "on build-1",
            "a row says where the workspace lives"
        );
        assert_eq!(Row::placed(Attention::Working, none, None).line, "");
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
        assert_eq!(Row::placed(Attention::NeedsYou, waiting, None).line, "2 need you");
        assert_eq!(
            Row::placed(Attention::Done, waiting, Some("build-1")).line,
            "2 need you \u{00B7} on build-1",
            "a count is never cut off, also when the workspace says where it lives"
        );
    }
}
