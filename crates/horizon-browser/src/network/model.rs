//! Public network-capture values and bounded option validation.

use serde::{Deserialize, Serialize};

use crate::BackendKind;

pub const DEFAULT_NETWORK_MAX_PAYLOAD_BYTES: u32 = 64 * 1024;
pub const DEFAULT_NETWORK_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_NETWORK_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_NETWORK_FILE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_NETWORK_URL_PATTERNS: usize = 32;
pub(super) const MAX_NETWORK_PATTERN_BYTES: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkOperation {
    Start,
    Status,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserNetworkCaptureOptions {
    pub include_http: bool,
    pub include_websocket: bool,
    pub frames: BrowserNetworkFrameOptions,
    pub url_patterns: Vec<String>,
    pub max_payload_bytes: u32,
    pub max_file_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserNetworkFrameOptions {
    pub include_sent: bool,
    pub include_received: bool,
}

impl Default for BrowserNetworkFrameOptions {
    fn default() -> Self {
        Self {
            include_sent: true,
            include_received: true,
        }
    }
}

impl Default for BrowserNetworkCaptureOptions {
    fn default() -> Self {
        Self {
            include_http: false,
            include_websocket: true,
            frames: BrowserNetworkFrameOptions::default(),
            url_patterns: Vec::new(),
            max_payload_bytes: DEFAULT_NETWORK_MAX_PAYLOAD_BYTES,
            max_file_bytes: DEFAULT_NETWORK_MAX_FILE_BYTES,
        }
    }
}

impl BrowserNetworkCaptureOptions {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if !self.include_http && !self.include_websocket {
            return Err("network capture must include HTTP, WebSocket, or both");
        }
        if self.include_websocket && !self.frames.include_sent && !self.frames.include_received {
            return Err("WebSocket capture must include sent frames, received frames, or both");
        }
        if self.url_patterns.len() > MAX_NETWORK_URL_PATTERNS {
            return Err("network capture has too many URL filters");
        }
        if self.url_patterns.iter().any(|pattern| {
            pattern.trim().is_empty()
                || pattern.len() > MAX_NETWORK_PATTERN_BYTES
                || pattern.chars().any(char::is_control)
        }) {
            return Err("network URL filters must be short printable substrings");
        }
        if self.max_payload_bytes > MAX_NETWORK_PAYLOAD_BYTES {
            return Err("network payload limit is too large");
        }
        if !(1..=MAX_NETWORK_FILE_BYTES).contains(&self.max_file_bytes) {
            return Err("network capture file limit is outside the supported range");
        }
        Ok(())
    }

    pub(crate) fn matches_url(&self, url: &str) -> bool {
        self.url_patterns.is_empty() || self.url_patterns.iter().any(|pattern| url.contains(pattern))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkEventKind {
    CaptureStarted,
    HttpRequest,
    HttpResponse,
    HttpCompleted,
    HttpFailed,
    WebsocketCreated,
    WebsocketOpened,
    WebsocketFrameSent,
    WebsocketFrameReceived,
    WebsocketError,
    WebsocketClosed,
    CaptureStopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkDirection {
    Sent,
    Received,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkPayloadEncoding {
    Text,
    Base64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserNetworkConnectionState {
    Connecting,
    Open,
    Closed,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserNetworkConnection {
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub state: BrowserNetworkConnectionState,
    pub observed_existing: bool,
    pub created_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_at_millis: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at_millis: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub sent_frames: u64,
    pub sent_bytes: u64,
    pub received_frames: u64,
    pub received_bytes: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserNetworkCapture {
    pub capture_id: String,
    pub path: String,
    pub active: bool,
    pub transport: String,
    pub records_enqueued: u64,
    pub records_written: u64,
    pub bytes_written: u64,
    pub records_dropped: u64,
    pub payloads_truncated: u64,
    pub file_limit_reached: bool,
    pub writer_failed: bool,
    pub known_connections: Vec<BrowserNetworkConnection>,
    pub connections_truncated: u64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct BrowserNetworkRecord {
    pub schema_version: u32,
    pub capture_id: String,
    pub sequence: u64,
    pub timestamp_millis: i64,
    pub backend: BackendKind,
    pub kind: BrowserNetworkEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<BrowserNetworkDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_encoding: Option<BrowserNetworkPayloadEncoding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl BrowserNetworkRecord {
    pub(crate) fn empty(capture_id: &str, sequence: u64, backend: BackendKind, kind: BrowserNetworkEventKind) -> Self {
        Self {
            schema_version: 1,
            capture_id: capture_id.to_string(),
            sequence,
            timestamp_millis: now_millis(),
            backend,
            kind,
            connection_id: None,
            url: None,
            method: None,
            status: None,
            resource_type: None,
            direction: None,
            opcode: None,
            payload: None,
            payload_encoding: None,
            payload_bytes: None,
            truncated: false,
            error: None,
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
