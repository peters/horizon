use horizon_browser::{
    BrowserControlAction, BrowserNetworkCapture, BrowserNetworkCaptureOptions, BrowserNetworkConnection,
    BrowserNetworkConnectionState, BrowserNetworkOperation,
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
