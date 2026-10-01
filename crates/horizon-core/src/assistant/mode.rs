//! How much an agent may do on its own: the permission mode it is started with.
//!
//! Every agent CLI has its own flags for this. The modes here are the same five steps for all of them,
//! and `args` turns a step into the flags of one agent, or says that the agent has no such step.

use serde::{Deserialize, Serialize};

use crate::PanelKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Read-only: it may look and think, and proposes a plan, but changes nothing.
    Plan,
    /// It asks before it changes or runs anything.
    #[default]
    Ask,
    /// File edits go through; commands still ask.
    AutoEdit,
    /// It works on its own inside the agent's own safety net (a sandbox or a reviewer).
    Auto,
    /// No questions and no safety net. For machines that are sandboxed from the outside.
    Yolo,
}

impl AgentMode {
    pub const ALL: [Self; 5] = [Self::Plan, Self::Ask, Self::AutoEdit, Self::Auto, Self::Yolo];

    /// The modes that do work, in order of how much they allow.
    pub const DOING: [Self; 4] = [Self::Ask, Self::AutoEdit, Self::Auto, Self::Yolo];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Ask => "Ask first",
            Self::AutoEdit => "Auto-edit",
            Self::Auto => "Auto",
            Self::Yolo => "YOLO",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Plan => "Read-only. Proposes a plan, changes nothing.",
            Self::Ask => "Asks before it edits files or runs commands.",
            Self::AutoEdit => "Edits files on its own; asks before running commands.",
            Self::Auto => "Works on its own inside the agent's safety net.",
            Self::Yolo => "No questions and no safety net. Only where nothing can be lost.",
        }
    }

    /// Whether the mode removes every check, so it deserves a second look before it is chosen.
    #[must_use]
    pub const fn removes_checks(self) -> bool {
        matches!(self, Self::Yolo)
    }

    /// The flags that start `kind` in this mode, or `None` when that agent has no such mode.
    /// `Some(vec![])` means the agent's own default is already this mode.
    #[must_use]
    pub fn args(self, kind: PanelKind) -> Option<Vec<String>> {
        let flags = |list: &[&str]| Some(list.iter().map(|flag| (*flag).to_string()).collect());
        match (kind, self) {
            // Every agent starts in its own default, which asks first.
            (_, Self::Ask) => flags(&[]),
            (PanelKind::Claude, Self::Plan) => flags(&["--permission-mode", "plan"]),
            (PanelKind::Claude, Self::AutoEdit) => flags(&["--permission-mode", "acceptEdits"]),
            (PanelKind::Claude, Self::Auto) => flags(&["--permission-mode", "auto"]),
            (PanelKind::Claude, Self::Yolo) => flags(&["--dangerously-skip-permissions"]),
            (PanelKind::Codex, Self::Plan) => flags(&["--sandbox", "read-only", "--ask-for-approval", "on-request"]),
            (PanelKind::Codex, Self::AutoEdit) => {
                flags(&["--sandbox", "workspace-write", "--ask-for-approval", "on-request"])
            }
            (PanelKind::Codex, Self::Auto) => flags(&["--sandbox", "workspace-write", "--ask-for-approval", "never"]),
            (PanelKind::Codex, Self::Yolo) => flags(&["--dangerously-bypass-approvals-and-sandbox"]),
            (PanelKind::Gemini, Self::AutoEdit | Self::Auto) => flags(&["--approval-mode", "auto_edit"]),
            (PanelKind::Gemini, Self::Yolo) => flags(&["--yolo"]),
            // Other agents have no flags for the rest here.
            _ => None,
        }
    }

    #[must_use]
    pub fn supported_by(self, kind: PanelKind) -> bool {
        self.args(kind).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::AgentMode;
    use crate::PanelKind;

    fn args(kind: PanelKind, mode: AgentMode) -> Vec<String> {
        mode.args(kind).expect("supported")
    }

    #[test]
    fn claude_modes_map_to_its_permission_flags() {
        assert_eq!(args(PanelKind::Claude, AgentMode::Plan), ["--permission-mode", "plan"]);
        assert!(args(PanelKind::Claude, AgentMode::Ask).is_empty());
        assert_eq!(args(PanelKind::Claude, AgentMode::Auto), ["--permission-mode", "auto"]);
        assert_eq!(
            args(PanelKind::Claude, AgentMode::Yolo),
            ["--dangerously-skip-permissions"]
        );
    }

    #[test]
    fn codex_plan_is_read_only_and_yolo_drops_the_sandbox() {
        let plan = args(PanelKind::Codex, AgentMode::Plan);
        assert!(plan.windows(2).any(|pair| pair == ["--sandbox", "read-only"]));
        assert_eq!(
            args(PanelKind::Codex, AgentMode::Yolo),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        // Auto never asks, but stays in the sandbox.
        let auto = args(PanelKind::Codex, AgentMode::Auto);
        assert!(auto.contains(&"never".to_string()) && auto.contains(&"workspace-write".to_string()));
    }

    #[test]
    fn an_agent_without_a_mode_says_so_but_always_has_ask() {
        assert!(AgentMode::Plan.args(PanelKind::Grok).is_none());
        assert!(AgentMode::Yolo.args(PanelKind::OpenCode).is_none());
        assert!(AgentMode::Ask.supported_by(PanelKind::Grok));
        assert!(!AgentMode::Plan.supported_by(PanelKind::Gemini));
    }

    #[test]
    fn only_yolo_removes_every_check() {
        assert_eq!(AgentMode::ALL.iter().filter(|mode| mode.removes_checks()).count(), 1);
    }

    #[test]
    fn modes_round_trip_through_json() {
        let text = serde_json::to_string(&AgentMode::AutoEdit).expect("serialize");
        assert_eq!(text, "\"auto_edit\"");
        assert_eq!(
            serde_json::from_str::<AgentMode>(&text).expect("parse"),
            AgentMode::AutoEdit
        );
    }
}
