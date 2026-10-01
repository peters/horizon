//! Agent coordination over the board: which agents exist in a workspace, what
//! each is doing, whether one can be sent a message, and what it printed.
//!
//! State comes from signals the board already tracks (the terminal's working
//! indicator, recent output, bracketed-paste mode and attention items), never
//! from interpreting the agent's reply.

use std::time::Duration;

use alacritty_terminal::term::TermMode;

use super::Board;
use crate::AgentStatus;
use crate::agents::agent_definition;
use crate::browser::manifest::agent_panels::{AgentPanel, AgentState};
use crate::panel::{Panel, PanelId, current_unix_millis};
use crate::workspace::WorkspaceId;

/// How long after its last output an agent still counts as busy. It is longer
/// than the working indicator's own stale window, so a fresh spinner is never missed.
const QUIET_BEFORE_SEND: Duration = Duration::from_millis(2500);
/// Matches the attention detector, which ignores a panel's first ten seconds.
const SETTLE_AFTER_LAUNCH_MS: i64 = 10_000;

/// Why a message cannot be typed into an agent right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendRefusal {
    UnknownPanel,
    NotAnAgent,
    Caller,
    Exited,
    Starting,
    Busy,
    NeedsInput,
}

impl SendRefusal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownPanel => "panel_unavailable",
            Self::NotAnAgent => "not_an_agent",
            Self::Caller => "cannot_send_to_self",
            Self::Exited => "agent_exited",
            Self::Starting => "agent_starting",
            Self::Busy => "agent_busy",
            Self::NeedsInput => "agent_needs_input",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnknownPanel => "No agent with that panel_id is in this workspace. List the agents first.",
            Self::NotAnAgent => "That panel is not an agent.",
            Self::Caller => "An agent cannot send a message to itself.",
            Self::Exited => "That agent's process has ended.",
            Self::Starting => "That agent is still starting. Wait until list shows it idle.",
            Self::Busy => "That agent is working. Wait until list shows it idle, then send.",
            Self::NeedsInput => {
                "That agent is waiting for a person to approve or answer something; typing now would answer it."
            }
        }
    }
}

/// What the board can observe about an agent, reduced to the facts that decide its state.
#[derive(Clone, Copy, Debug)]
struct Observation {
    exited: bool,
    interface_up: bool,
    active: bool,
    needs_input: bool,
}

fn classify(observation: Observation) -> AgentState {
    if observation.exited {
        AgentState::Exited
    } else if !observation.interface_up {
        AgentState::Starting
    } else if observation.needs_input {
        AgentState::NeedsInput
    } else if observation.active {
        AgentState::Working
    } else {
        AgentState::Idle
    }
}

impl Board {
    fn agent_state_of(&self, panel: &Panel) -> AgentState {
        let Some(terminal) = panel.terminal() else {
            return AgentState::Exited;
        };
        // The attention feed can be switched off or dismissed by the person, so
        // the screen is read directly: a pending approval or question must never
        // look idle.
        let asking = self
            .unresolved_attention_for_panel(panel.id)
            .is_some_and(|item| item.source == "agent" && !item.is_agent_ready_for_input())
            || matches!(
                panel.detect_attention(),
                Some("Waiting for approval" | "Waiting for input")
            );
        // Attention detection is blind for the first seconds after launch, so a
        // young agent is still starting whatever its screen shows.
        let settled = current_unix_millis().saturating_sub(panel.launched_at_millis) >= SETTLE_AFTER_LAUNCH_MS;
        classify(Observation {
            exited: terminal.child_exited(),
            // Agent TUIs turn bracketed paste on once their prompt is up.
            interface_up: settled && terminal.mode().contains(TermMode::BRACKETED_PASTE),
            active: panel.agent_status() == AgentStatus::Working || panel.had_recent_output_within(QUIET_BEFORE_SEND),
            needs_input: asking,
        })
    }

    /// The agents in `workspace`, marking the one that asked. The assistant is
    /// the person's own conversation, so other agents never see it.
    #[must_use]
    pub fn agent_panels_in_workspace(&self, workspace: WorkspaceId, caller: PanelId) -> Vec<AgentPanel> {
        self.panels
            .iter()
            .filter(|panel| panel.workspace_id == workspace && panel.kind.is_agent())
            .filter(|panel| !panel.is_assistant() || panel.id == caller)
            .map(|panel| AgentPanel {
                panel_id: panel.local_id.clone(),
                title: panel.display_title().into_owned(),
                kind: agent_definition(panel.kind).map_or_else(String::new, |agent| agent.id.to_string()),
                state: self.agent_state_of(panel),
                directory: panel.launch_cwd.as_ref().map(|path| path.display().to_string()),
                is_caller: panel.id == caller,
            })
            .collect()
    }

    /// The state of one agent panel, if it is an agent.
    #[must_use]
    pub fn agent_state(&self, panel_id: PanelId) -> Option<AgentState> {
        self.panel(panel_id)
            .filter(|panel| panel.kind.is_agent())
            .map(|panel| self.agent_state_of(panel))
    }

    /// Whether `caller` may type a message into `target` right now.
    ///
    /// # Errors
    /// Returns the reason the message must not be sent.
    pub fn check_agent_can_receive(&self, caller: PanelId, target: PanelId) -> Result<(), SendRefusal> {
        if target == caller {
            return Err(SendRefusal::Caller);
        }
        let Some(panel) = self.panel(target) else {
            return Err(SendRefusal::UnknownPanel);
        };
        if !panel.kind.is_agent() {
            return Err(SendRefusal::NotAnAgent);
        }
        match self.agent_state_of(panel) {
            AgentState::Idle => Ok(()),
            AgentState::Exited => Err(SendRefusal::Exited),
            AgentState::Starting => Err(SendRefusal::Starting),
            AgentState::Working => Err(SendRefusal::Busy),
            AgentState::NeedsInput => Err(SendRefusal::NeedsInput),
        }
    }

    /// The newest `lines` rows of an agent's terminal, oldest first, and whether
    /// older rows were left out.
    #[must_use]
    pub fn agent_output(&self, panel_id: PanelId, lines: usize) -> Option<(String, bool)> {
        let panel = self.panel(panel_id).filter(|panel| panel.kind.is_agent())?;
        let terminal = panel.terminal()?;
        // Asking for fewer rows than the screen is tall returns its top, so ask
        // for the screen on top of the lines wanted and keep the newest of them.
        let (rows, _) = terminal.full_text_lines(lines + usize::from(terminal.rows()));
        let truncated = rows.len() > lines;
        let newest = &rows[rows.len().saturating_sub(lines)..];
        Some((newest.join("\n"), truncated))
    }
}

#[cfg(test)]
mod tests;
