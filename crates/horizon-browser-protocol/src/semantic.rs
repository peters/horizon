use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserTarget {
    Ref { reference: String },
    Selector { selector: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BrowserNode {
    #[serde(rename = "ref")]
    pub reference: String,
    pub role: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub visible: bool,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<BrowserBounds>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BrowserSnapshot {
    pub generation: u64,
    pub revision: u64,
    pub url: String,
    pub title: String,
    pub nodes: Vec<BrowserNode>,
}

/// Selector condition a wait action observes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorState {
    /// At least one element matches.
    Present,
    /// At least one matching element is rendered visible.
    Visible,
    /// No matching element is visible (an empty match set counts).
    Hidden,
}

impl SelectorState {
    #[must_use]
    pub fn satisfied_by(self, nodes: &[BrowserNode]) -> bool {
        match self {
            Self::Present => !nodes.is_empty(),
            Self::Visible => nodes.iter().any(|node| node.visible),
            Self::Hidden => nodes.iter().all(|node| !node.visible),
        }
    }
}

/// Result of a satisfied wait action. Timeouts and cancellations are failed
/// actions with typed codes (`wait_timeout`, `wait_navigation_invalidated`,
/// `wait_ownership_lost`, `wait_handoff_pending`, `wait_superseded`,
/// `browser_unavailable`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct WaitOutcome {
    pub state: SelectorState,
    pub generation: u64,
    pub revision: u64,
    /// Nodes matching the selector when the condition was met.
    pub nodes: Vec<BrowserNode>,
    /// Time from the caller queuing the action until the condition was met.
    pub elapsed_millis: u64,
    /// Page observations the engine needed.
    pub polls: u32,
}

/// Where a navigation action stood when its outcome was reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationState {
    /// The command was handed to the backend; its acceptance was not awaited
    /// and nothing about the page is known yet.
    Dispatched,
    /// The top-level document committed; `committed_url` is authoritative.
    Committed,
    /// The committed document fired `DOMContentLoaded`.
    DomContentLoaded,
    /// The requested readiness did not arrive within the bound; the fields
    /// carry the latest page state the engine observed.
    TimedOut,
    /// A later navigation action replaced this one before it settled.
    Superseded,
}

/// Typed result of a navigation action. Failures (unreachable destination,
/// rejected command) are reported as a failed action, not as a state here.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct NavigationOutcome {
    /// Destination after omnibox normalization.
    pub requested_url: String,
    /// Readiness the caller asked to wait for.
    pub wait: crate::NavigationWait,
    pub state: NavigationState,
    /// URL of the committed top-level document, when one committed during
    /// this action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_url: Option<String>,
    /// Title observed for the committed document, when already known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Whether the committed document was still loading when reported.
    pub loading: bool,
    /// The committed URL differs from the requested destination.
    pub redirected: bool,
    pub elapsed_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserControlValue {
    Accepted,
    Navigation {
        navigation: NavigationOutcome,
    },
    Wait {
        wait: WaitOutcome,
    },
    Snapshot {
        snapshot: BrowserSnapshot,
    },
    Nodes {
        generation: u64,
        revision: u64,
        nodes: Vec<BrowserNode>,
    },
    Json {
        value: Value,
    },
    Network {
        capture: crate::BrowserNetworkCapture,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserControlFailure {
    pub code: String,
    pub message: String,
}

impl BrowserControlFailure {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserActionOutcome {
    Completed { value: BrowserControlValue },
    Failed { error: BrowserControlFailure },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct AgentActionResult {
    pub schema_version: u32,
    pub action_id: String,
    pub completed_at_millis: i64,
    #[serde(flatten)]
    pub outcome: BrowserActionOutcome,
}

impl AgentActionResult {
    #[must_use]
    pub fn completed(action_id: impl Into<String>, value: BrowserControlValue) -> Self {
        Self {
            schema_version: 1,
            action_id: action_id.into(),
            completed_at_millis: now_millis(),
            outcome: BrowserActionOutcome::Completed { value },
        }
    }

    #[must_use]
    pub fn failed(action_id: impl Into<String>, error: BrowserControlFailure) -> Self {
        Self {
            schema_version: 1,
            action_id: action_id.into(),
            completed_at_millis: now_millis(),
            outcome: BrowserActionOutcome::Failed { error },
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
