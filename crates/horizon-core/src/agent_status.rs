use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Rich status for AI agent panels, replacing the crude `detect_attention()`
/// pattern matching with a typed state model that drives both the UI and
/// programmatic orchestration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Panel is less than 10 seconds old; output is likely startup noise.
    Launching,
    /// Agent is producing output and has not reached a prompt.
    Working,
    /// Agent is asking for permission ("Allow ...", "[y/N]", etc.).
    WaitingForApproval,
    /// Agent is asking a question (line ends with `?`).
    WaitingForInput,
    /// Agent is at a prompt (`>`, `>>>`) and ready to accept input.
    Idle,
    /// The child process has exited.
    Exited,
}

impl AgentStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Launching => "Launching",
            Self::Working => "Working",
            Self::WaitingForApproval => "Waiting for approval",
            Self::WaitingForInput => "Waiting for input",
            Self::Idle => "Idle",
            Self::Exited => "Exited",
        }
    }

    /// Returns `true` when the agent is in a state where user interaction is
    /// expected or possible (i.e. it is not actively processing).
    #[must_use]
    pub const fn is_actionable(self) -> bool {
        matches!(
            self,
            Self::WaitingForApproval | Self::WaitingForInput | Self::Idle
        )
    }
}

/// A point-in-time snapshot of an agent's status together with timing
/// information for duration tracking and transition detection.
#[derive(Clone, Debug)]
pub struct AgentStateSnapshot {
    pub status: AgentStatus,
    pub entered_at: Instant,
    pub previous: Option<AgentStatus>,
}

impl AgentStateSnapshot {
    #[must_use]
    pub fn new(status: AgentStatus) -> Self {
        Self {
            status,
            entered_at: Instant::now(),
            previous: None,
        }
    }

    /// Transition to a new status, recording the old one.
    pub fn transition(&mut self, next: AgentStatus) {
        self.previous = Some(self.status);
        self.status = next;
        self.entered_at = Instant::now();
    }

    /// How long the agent has been in the current status.
    #[must_use]
    pub fn duration(&self) -> std::time::Duration {
        self.entered_at.elapsed()
    }
}

/// Derive an [`AgentStatus`] from the last few lines of terminal output.
///
/// The pattern matching mirrors the logic that previously lived in
/// `Panel::detect_attention()` but produces a richer enum instead of an
/// `Option<&'static str>`.
#[must_use]
pub fn derive_agent_status(last_lines: &str, child_exited: bool, age_ms: i64) -> AgentStatus {
    if child_exited {
        return AgentStatus::Exited;
    }
    if age_ms < 10_000 {
        return AgentStatus::Launching;
    }
    if last_lines.is_empty() {
        return AgentStatus::Working;
    }

    for line in last_lines.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with("Allow")
            || trimmed.starts_with("Do you want")
            || trimmed.ends_with("[y/N]")
            || trimmed.ends_with("[Y/n]")
            || trimmed.ends_with("(y/n)")
        {
            return AgentStatus::WaitingForApproval;
        }

        if trimmed.ends_with('?') && trimmed.len() > 2 {
            return AgentStatus::WaitingForInput;
        }

        if trimmed.starts_with('>') || trimmed.starts_with('\u{276F}') {
            return AgentStatus::Idle;
        }

        // First non-empty line didn't match any prompt pattern.
        break;
    }

    AgentStatus::Working
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_takes_priority() {
        assert_eq!(derive_agent_status("some output", true, 20_000), AgentStatus::Exited);
    }

    #[test]
    fn launching_when_young() {
        assert_eq!(derive_agent_status("> ", false, 5_000), AgentStatus::Launching);
    }

    #[test]
    fn working_on_empty_output() {
        assert_eq!(derive_agent_status("", false, 20_000), AgentStatus::Working);
    }

    #[test]
    fn idle_on_prompt() {
        assert_eq!(derive_agent_status("> ", false, 20_000), AgentStatus::Idle);
        assert_eq!(derive_agent_status("\u{276F} ", false, 20_000), AgentStatus::Idle);
    }

    #[test]
    fn waiting_for_approval() {
        assert_eq!(
            derive_agent_status("Allow read access to /home? [y/N]", false, 20_000),
            AgentStatus::WaitingForApproval,
        );
        assert_eq!(
            derive_agent_status("Do you want to continue?", false, 20_000),
            AgentStatus::WaitingForApproval,
        );
    }

    #[test]
    fn waiting_for_input_on_question() {
        assert_eq!(
            derive_agent_status("What file should I edit?", false, 20_000),
            AgentStatus::WaitingForInput,
        );
    }

    #[test]
    fn working_on_normal_output() {
        assert_eq!(
            derive_agent_status("Compiling horizon-core v0.1.0", false, 20_000),
            AgentStatus::Working,
        );
    }

    #[test]
    fn transition_records_previous() {
        let mut snap = AgentStateSnapshot::new(AgentStatus::Launching);
        assert_eq!(snap.previous, None);

        snap.transition(AgentStatus::Working);
        assert_eq!(snap.status, AgentStatus::Working);
        assert_eq!(snap.previous, Some(AgentStatus::Launching));

        snap.transition(AgentStatus::Idle);
        assert_eq!(snap.status, AgentStatus::Idle);
        assert_eq!(snap.previous, Some(AgentStatus::Working));
    }

    #[test]
    fn actionable_states() {
        assert!(AgentStatus::WaitingForApproval.is_actionable());
        assert!(AgentStatus::WaitingForInput.is_actionable());
        assert!(AgentStatus::Idle.is_actionable());
        assert!(!AgentStatus::Working.is_actionable());
        assert!(!AgentStatus::Launching.is_actionable());
        assert!(!AgentStatus::Exited.is_actionable());
    }

    #[test]
    fn multiline_last_lines_checks_last_non_empty() {
        let text = "some output\n\n> ";
        assert_eq!(derive_agent_status(text, false, 20_000), AgentStatus::Idle);
    }

    #[test]
    fn approval_patterns_cover_y_n_variants() {
        assert_eq!(
            derive_agent_status("Continue? [Y/n]", false, 20_000),
            AgentStatus::WaitingForApproval,
        );
        assert_eq!(
            derive_agent_status("Proceed (y/n)", false, 20_000),
            AgentStatus::WaitingForApproval,
        );
    }
}
