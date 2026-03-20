use serde::{Deserialize, Serialize};

use crate::github::GitHubWorkItemRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskRole {
    Research,
    Implement,
    Review,
}

impl TaskRole {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Research => "Research",
            Self::Implement => "Implement",
            Self::Review => "Review",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWaitStatus {
    #[default]
    Running,
    NeedsInput,
    NeedsReview,
    Blocked,
    Done,
}

impl TaskWaitStatus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::NeedsInput => "Needs input",
            Self::NeedsReview => "Needs review",
            Self::Blocked => "Blocked",
            Self::Done => "Done",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPrState {
    #[default]
    None,
    Draft {
        number: u64,
    },
    Open {
        number: u64,
    },
    Merged {
        number: u64,
    },
    Closed {
        number: u64,
    },
}

impl TaskPrState {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::None => "No PR".to_string(),
            Self::Draft { number } => format!("PR #{number} draft"),
            Self::Open { number } => format!("PR #{number} open"),
            Self::Merged { number } => format!("PR #{number} merged"),
            Self::Closed { number } => format!("PR #{number} closed"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct TaskPanelStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub pr_state: TaskPrState,
    #[serde(default)]
    pub wait_status: TaskWaitStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TaskWorkspaceBinding {
    pub task_id: String,
    pub work_item: GitHubWorkItemRef,
    pub repo_root: String,
}

impl TaskWorkspaceBinding {
    #[must_use]
    pub fn label(&self) -> String {
        self.work_item.label()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskUsageSummary {
    pub task_id: String,
    pub label: String,
    pub claude_sessions: u32,
    pub claude_tokens: u64,
    pub claude_messages: u32,
    pub codex_sessions: u32,
    pub codex_tokens: u64,
}

impl TaskUsageSummary {
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.claude_tokens.saturating_add(self.codex_tokens)
    }
}
