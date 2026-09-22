//! Cloud-worker presentation protocol transported only over authenticated SSH.
pub const MAX_CLOUD_VIEW_BYTES: usize = 96 * 1024 * 1024;
use crate::BrowserCommand;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum CloudViewRequest {
    Open {
        id: String,
        url: Option<String>,
        #[serde(default)]
        backend: Option<crate::BackendKind>,
        #[serde(default)]
        target: Option<String>,
    },
    Poll {
        id: String,
        after: u64,
        commands: Vec<BrowserCommand>,
    },
    List,
    RevokeRemote,
    Close {
        id: String,
    },
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CloudViewState {
    pub id: String,
    #[serde(default)]
    pub backend: crate::BackendKind,
    #[serde(default)]
    pub remote_target: Option<String>,
    #[serde(default)]
    pub remote_device: Option<String>,
    pub title: String,
    pub url: String,
    pub owner: Option<String>,
    pub ready: bool,
    pub visible: bool,
    pub handoff: Option<String>,
    #[serde(default)]
    pub handoff_sequence: u64,
    #[serde(default)]
    pub handoff_error: Option<String>,
    pub lost: bool,
    pub sequence: u64,
    pub png: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CloudViewResponse {
    #[serde(default)]
    pub closed: Vec<String>,
    #[serde(default)]
    pub desktop_controller: Option<String>,
    #[serde(default)]
    pub desktop_last_input: Option<String>,
    pub browsers: Vec<CloudViewState>,
    pub error: Option<String>,
}
