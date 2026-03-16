use std::time::SystemTime;

use crate::{PanelId, WorkspaceId};

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

#[derive(Clone, Debug)]
pub struct AttentionItem {
    pub id: AttentionId,
    pub workspace_id: WorkspaceId,
    pub panel_id: Option<PanelId>,
    pub source: String,
    pub summary: String,
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
        self.source == "agent" && self.summary == "Ready for input"
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
