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

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserControlValue {
    Accepted,
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
