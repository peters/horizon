use std::time::SystemTime;

use crate::{PanelId, TaskRole, WorkspaceId};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct AttentionId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum AttentionSeverity {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttentionState {
    Open,
    Resolved,
    Dismissed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttentionKind {
    Generic,
    InputRequested,
    ReviewRequested,
    Blocked,
    Completed,
}

#[derive(Clone, Debug)]
pub struct AttentionItem {
    pub id: AttentionId,
    pub workspace_id: WorkspaceId,
    pub panel_id: Option<PanelId>,
    pub source: String,
    pub summary: String,
    pub kind: AttentionKind,
    pub task_label: Option<String>,
    pub task_role: Option<TaskRole>,
    pub severity: AttentionSeverity,
    pub state: AttentionState,
    pub created_at: SystemTime,
    pub resolved_at: Option<SystemTime>,
}

impl AttentionItem {
    #[must_use]
    pub fn new(
        id: AttentionId,
        workspace_id: WorkspaceId,
        panel_id: Option<PanelId>,
        source: impl Into<String>,
        summary: impl Into<String>,
        severity: AttentionSeverity,
    ) -> Self {
        Self {
            id,
            workspace_id,
            panel_id,
            source: source.into(),
            summary: summary.into(),
            kind: AttentionKind::Generic,
            task_label: None,
            task_role: None,
            severity,
            state: AttentionState::Open,
            created_at: SystemTime::now(),
            resolved_at: None,
        }
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state == AttentionState::Open
    }

    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.state == AttentionState::Resolved
    }

    #[must_use]
    pub fn is_agent_ready_for_input(&self) -> bool {
        self.kind == AttentionKind::Generic && self.source == "agent" && self.summary == "Ready for input"
    }

    #[must_use]
    pub fn with_task(mut self, kind: AttentionKind, task_label: Option<String>, task_role: Option<TaskRole>) -> Self {
        self.kind = kind;
        self.task_label = task_label;
        self.task_role = task_role;
        self
    }

    pub fn resolve(&mut self) {
        if self.is_open() {
            self.state = AttentionState::Resolved;
            self.resolved_at = Some(SystemTime::now());
        }
    }

    pub fn dismiss(&mut self) {
        if self.is_open() {
            self.state = AttentionState::Dismissed;
            self.resolved_at = Some(SystemTime::now());
        }
    }
}
