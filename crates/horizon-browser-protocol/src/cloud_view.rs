//! Cloud-worker presentation protocol transported only over authenticated SSH.
pub const MAX_CLOUD_VIEW_BYTES: usize = 96 * 1024 * 1024;
use crate::BrowserCommand;
use serde::{Deserialize, Serialize};
mod uploads;
pub use uploads::{
    CLOUD_UPLOAD_VERSION, CloudBrowserNotice, CloudChooserAnswer, CloudChooserError, CloudChooserRequest,
    CloudFileDrop, CloudStageChunk, CloudStageReceipt, CloudStagedFiles, CloudUploadLimits, CloudUploadPoll,
    CloudUploadState, MAX_UPLOAD_NAME_BYTES, valid_upload_batch, valid_upload_name,
};
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uploads: Option<CloudUploadPoll>,
    },
    Stage(CloudStageChunk),
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
    #[serde(default)]
    pub orientation: crate::remote::RemoteOrientationView,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploads: Option<CloudUploadState>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<CloudStageReceipt>,
}
