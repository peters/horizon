use horizon_browser::{
    BrowserControlAction, BrowserNetworkCapture, BrowserNetworkCaptureOptions, BrowserNetworkConnection,
    BrowserNetworkConnectionState, BrowserNetworkEventKind, BrowserNetworkOperation, BrowserNetworkRecord,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum NetworkOperation {
    Start,
    Status,
    Stop,
}

impl From<NetworkOperation> for BrowserNetworkOperation {
    fn from(value: NetworkOperation) -> Self {
        match value {
            NetworkOperation::Start => Self::Start,
            NetworkOperation::Status => Self::Status,
            NetworkOperation::Stop => Self::Stop,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NetworkInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Start a new capture, inspect the active/last capture, or stop and flush it.
    operation: NetworkOperation,
    /// Include bounded HTTP lifecycle metadata (default false). Start only.
    include_http: Option<bool>,
    /// Include bounded HTTP response bodies through native browser protocol support (default false). Start only.
    include_http_bodies: Option<bool>,
    /// Include WebSocket lifecycle and frames (default true). Start only.
    include_websocket: Option<bool>,
    /// Capture outbound WebSocket frames (default true). Start only.
    include_sent: Option<bool>,
    /// Capture inbound WebSocket frames (default true). Start only.
    include_received: Option<bool>,
    /// Optional URL substring filters, applied before payloads cross the capture boundary. Start only.
    url_patterns: Option<Vec<String>>,
    /// Maximum stored bytes per payload (default 65536, maximum 1048576). Start only.
    max_payload_bytes: Option<u32>,
    /// Maximum NDJSON file size in bytes (default 134217728, maximum 1073741824). Start only.
    max_file_bytes: Option<u64>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

impl NetworkInput {
    pub(crate) fn build_action(&self) -> Result<BrowserControlAction, String> {
        let has_options = self.include_http.is_some()
            || self.include_http_bodies.is_some()
            || self.include_websocket.is_some()
            || self.include_sent.is_some()
            || self.include_received.is_some()
            || self.url_patterns.is_some()
            || self.max_payload_bytes.is_some()
            || self.max_file_bytes.is_some();
        if !matches!(self.operation, NetworkOperation::Start) && has_options {
            return Err("network status and stop do not accept capture options".to_string());
        }
        let options = if matches!(self.operation, NetworkOperation::Start) {
            let mut options = BrowserNetworkCaptureOptions::default();
            if let Some(value) = self.include_http {
                options.include_http = value;
            }
            if let Some(value) = self.include_http_bodies {
                options.include_http_bodies = value;
            }
            if let Some(value) = self.include_websocket {
                options.include_websocket = value;
            }
            if let Some(value) = self.include_sent {
                options.frames.include_sent = value;
            }
            if let Some(value) = self.include_received {
                options.frames.include_received = value;
            }
            if let Some(value) = &self.url_patterns {
                options.url_patterns.clone_from(value);
            }
            if let Some(value) = self.max_payload_bytes {
                options.max_payload_bytes = value;
            }
            if let Some(value) = self.max_file_bytes {
                options.max_file_bytes = value;
            }
            Some(options)
        } else {
            None
        };
        Ok(BrowserControlAction::Network {
            operation: self.operation.into(),
            options,
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct NetworkConnectionOutput {
    connection_id: String,
    url: Option<String>,
    state: String,
    observed_existing: bool,
    created_at_millis: i64,
    opened_at_millis: Option<i64>,
    closed_at_millis: Option<i64>,
    last_error: Option<String>,
    sent_frames: u64,
    sent_bytes: u64,
    received_frames: u64,
    received_bytes: u64,
    last_sequence: u64,
}

impl From<BrowserNetworkConnection> for NetworkConnectionOutput {
    fn from(value: BrowserNetworkConnection) -> Self {
        Self {
            connection_id: value.connection_id,
            url: value.url,
            state: match value.state {
                BrowserNetworkConnectionState::Connecting => "connecting",
                BrowserNetworkConnectionState::Open => "open",
                BrowserNetworkConnectionState::Closed => "closed",
                BrowserNetworkConnectionState::Error => "error",
            }
            .to_string(),
            observed_existing: value.observed_existing,
            created_at_millis: value.created_at_millis,
            opened_at_millis: value.opened_at_millis,
            closed_at_millis: value.closed_at_millis,
            last_error: value.last_error,
            sent_frames: value.sent_frames,
            sent_bytes: value.sent_bytes,
            received_frames: value.received_frames,
            received_bytes: value.received_bytes,
            last_sequence: value.last_sequence,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkOutput {
    panel_id: String,
    action_id: String,
    capture_id: String,
    /// Private local NDJSON export. Agents may use ordinary read-only Unix tools on this exact returned path.
    path: String,
    active: bool,
    transport: String,
    records_enqueued: u64,
    records_written: u64,
    bytes_written: u64,
    records_dropped: u64,
    payloads_truncated: u64,
    file_limit_reached: bool,
    writer_failed: bool,
    known_connections: Vec<NetworkConnectionOutput>,
    connections_truncated: u64,
    next_step: String,
}

impl NetworkOutput {
    pub(crate) fn new(panel_id: String, action_id: String, capture: BrowserNetworkCapture) -> Self {
        Self {
            panel_id,
            action_id,
            capture_id: capture.capture_id,
            path: capture.path,
            active: capture.active,
            transport: capture.transport,
            records_enqueued: capture.records_enqueued,
            records_written: capture.records_written,
            bytes_written: capture.bytes_written,
            records_dropped: capture.records_dropped,
            payloads_truncated: capture.payloads_truncated,
            file_limit_reached: capture.file_limit_reached,
            writer_failed: capture.writer_failed,
            known_connections: capture
                .known_connections
                .into_iter()
                .map(NetworkConnectionOutput::from)
                .collect(),
            connections_truncated: capture.connections_truncated,
            next_step: if capture.active {
                "Inspect browser_network status or tail -f this exact path; call browser_network stop when done."
                    .to_string()
            } else {
                "The capture is flushed; inspect this exact path with jq, rg, tail, or another read-only tool."
                    .to_string()
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NetworkWatchEventKind {
    CaptureStarted,
    HttpRequest,
    HttpResponse,
    HttpResponseBody,
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

impl NetworkWatchEventKind {
    pub(crate) const fn matches(self, kind: BrowserNetworkEventKind) -> bool {
        matches!(
            (self, kind),
            (Self::CaptureStarted, BrowserNetworkEventKind::CaptureStarted)
                | (Self::HttpRequest, BrowserNetworkEventKind::HttpRequest)
                | (Self::HttpResponse, BrowserNetworkEventKind::HttpResponse)
                | (Self::HttpResponseBody, BrowserNetworkEventKind::HttpResponseBody)
                | (Self::HttpCompleted, BrowserNetworkEventKind::HttpCompleted)
                | (Self::HttpFailed, BrowserNetworkEventKind::HttpFailed)
                | (Self::WebsocketCreated, BrowserNetworkEventKind::WebsocketCreated)
                | (Self::WebsocketOpened, BrowserNetworkEventKind::WebsocketOpened)
                | (Self::WebsocketFrameSent, BrowserNetworkEventKind::WebsocketFrameSent)
                | (
                    Self::WebsocketFrameReceived,
                    BrowserNetworkEventKind::WebsocketFrameReceived
                )
                | (Self::WebsocketError, BrowserNetworkEventKind::WebsocketError)
                | (Self::WebsocketClosed, BrowserNetworkEventKind::WebsocketClosed)
                | (Self::CaptureStopped, BrowserNetworkEventKind::CaptureStopped)
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NetworkWatchInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Optional capture id from the previous response. A changed capture resets the cursor explicitly.
    pub(crate) capture_id: Option<String>,
    /// Return records strictly after this monotonic capture sequence (default 0).
    pub(crate) after_sequence: Option<u64>,
    /// Bounded long-poll duration in milliseconds (1-60000, default 60000).
    pub(crate) wait_millis: Option<u64>,
    /// Maximum matching records returned at once (1-250, default 100).
    pub(crate) max_records: Option<u32>,
    /// Optional URL substring filters applied before records are returned.
    pub(crate) url_patterns: Option<Vec<String>>,
    /// Optional event kinds. Omit or pass an empty list to accept every kind.
    pub(crate) event_kinds: Option<Vec<NetworkWatchEventKind>>,
    /// Include bounded payload text (default false).
    pub(crate) include_payload: Option<bool>,
    /// Aggregate payload-byte budget for the whole response (1-65536, default
    /// 16384). Oversized payloads are metadata-only in multi-record batches;
    /// use `max_records=1` for a bounded prefix. Requires `include_payload=true`.
    pub(crate) max_payload_bytes: Option<u32>,
    /// Per-status action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkWatchRecord {
    pub(crate) sequence: u64,
    pub(crate) timestamp_millis: i64,
    pub(crate) backend: String,
    pub(crate) kind: String,
    pub(crate) connection_id: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) method: Option<String>,
    pub(crate) status: Option<u16>,
    pub(crate) resource_type: Option<String>,
    pub(crate) direction: Option<String>,
    pub(crate) opcode: Option<u8>,
    pub(crate) payload: Option<String>,
    pub(crate) payload_encoding: Option<String>,
    pub(crate) payload_bytes: Option<u64>,
    pub(crate) truncated: bool,
    pub(crate) error: Option<String>,
}

impl NetworkWatchRecord {
    pub(crate) fn from_record(
        mut value: BrowserNetworkRecord,
        include_payload: bool,
        max_payload_bytes: usize,
    ) -> Self {
        let watcher_truncated = value.payload.as_mut().is_some_and(|payload| {
            if !include_payload {
                payload.clear();
                return false;
            }
            if payload.len() <= max_payload_bytes {
                return false;
            }
            let mut boundary = max_payload_bytes;
            while boundary > 0 && !payload.is_char_boundary(boundary) {
                boundary -= 1;
            }
            payload.truncate(boundary);
            true
        });
        let payload = include_payload.then_some(value.payload).flatten();
        Self {
            sequence: value.sequence,
            timestamp_millis: value.timestamp_millis,
            backend: super::backend_name(value.backend).to_string(),
            kind: event_kind_name(value.kind).to_string(),
            connection_id: value.connection_id,
            url: value.url,
            method: value.method,
            status: value.status,
            resource_type: value.resource_type,
            direction: value.direction.map(|direction| match direction {
                horizon_browser::BrowserNetworkDirection::Sent => "sent".to_string(),
                horizon_browser::BrowserNetworkDirection::Received => "received".to_string(),
            }),
            opcode: value.opcode,
            payload,
            payload_encoding: value.payload_encoding.map(|encoding| match encoding {
                horizon_browser::BrowserNetworkPayloadEncoding::Text => "text".to_string(),
                horizon_browser::BrowserNetworkPayloadEncoding::Base64 => "base64".to_string(),
            }),
            payload_bytes: value.payload_bytes,
            truncated: value.truncated || watcher_truncated,
            error: value.error,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkWatchOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) capture_id: String,
    pub(crate) records: Vec<NetworkWatchRecord>,
    pub(crate) next_sequence: u64,
    #[serde(flatten)]
    pub(crate) delivery: NetworkWatchDeliveryState,
    pub(crate) sequence_gaps: u64,
    pub(crate) malformed_records: u64,
    pub(crate) connection_urls_truncated: u64,
    pub(crate) records_dropped: u64,
    pub(crate) payloads_truncated: u64,
    pub(crate) returned_payloads_truncated: u64,
    #[serde(flatten)]
    pub(crate) capture_state: NetworkWatchCaptureState,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkWatchDeliveryState {
    pub(crate) timed_out: bool,
    pub(crate) capture_changed: bool,
    pub(crate) file_reset: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkWatchCaptureState {
    pub(crate) capture_active: bool,
    pub(crate) file_limit_reached: bool,
    pub(crate) writer_failed: bool,
}

pub(crate) const fn event_kind_name(kind: BrowserNetworkEventKind) -> &'static str {
    match kind {
        BrowserNetworkEventKind::CaptureStarted => "capture_started",
        BrowserNetworkEventKind::HttpRequest => "http_request",
        BrowserNetworkEventKind::HttpResponse => "http_response",
        BrowserNetworkEventKind::HttpResponseBody => "http_response_body",
        BrowserNetworkEventKind::HttpCompleted => "http_completed",
        BrowserNetworkEventKind::HttpFailed => "http_failed",
        BrowserNetworkEventKind::WebsocketCreated => "websocket_created",
        BrowserNetworkEventKind::WebsocketOpened => "websocket_opened",
        BrowserNetworkEventKind::WebsocketFrameSent => "websocket_frame_sent",
        BrowserNetworkEventKind::WebsocketFrameReceived => "websocket_frame_received",
        BrowserNetworkEventKind::WebsocketError => "websocket_error",
        BrowserNetworkEventKind::WebsocketClosed => "websocket_closed",
        BrowserNetworkEventKind::CaptureStopped => "capture_stopped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_body_option_is_part_of_the_single_network_contract() {
        let input = serde_json::from_value::<NetworkInput>(serde_json::json!({
            "panel_id": "panel-1",
            "operation": "start",
            "include_http": true,
            "include_http_bodies": true,
            "include_websocket": false
        }))
        .unwrap_or_else(|error| panic!("network input failed to decode: {error}"));
        let action = input
            .build_action()
            .unwrap_or_else(|error| panic!("network action failed to build: {error}"));
        let BrowserControlAction::Network { options, .. } = action else {
            panic!("network input built the wrong action");
        };
        let options = options.unwrap_or_else(|| panic!("start action omitted capture options"));

        assert!(options.include_http);
        assert!(options.include_http_bodies);
        assert!(!options.include_websocket);
    }
}
