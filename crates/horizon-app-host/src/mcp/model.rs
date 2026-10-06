use horizon_app_testing::{
    contract::Platform,
    recipe::{Action, State, Target},
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub platform: Platform,
    pub lifetime_seconds: u64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub lifetime_seconds: u64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub artifact: String,
    pub matrix_index: usize,
    pub lifetime_seconds: u64,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub session: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Logs {
    pub session: String,
    pub kind: LogKind,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    Device,
    Crash,
    Appium,
    Network,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Video {
    pub session: String,
    pub operation: VideoOperation,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VideoOperation {
    Start,
    Status,
    Get,
    Stop,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub after_sequence: u64,
    pub limit: usize,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Act {
    pub session: String,
    pub action: Action,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    pub session: String,
    pub target: Target,
    pub state: State,
    pub timeout_millis: u64,
}
