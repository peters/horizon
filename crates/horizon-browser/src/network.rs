//! Backend-neutral, bounded network observation and private NDJSON export.

mod model;
mod writer;

use std::collections::HashMap;
use std::path::Path;

use crate::{BackendKind, BrowserControlFailure};
#[cfg(test)]
use model::MAX_NETWORK_PATTERN_BYTES;
pub use model::{
    BrowserNetworkCapture, BrowserNetworkCaptureOptions, BrowserNetworkConnection, BrowserNetworkConnectionState,
    BrowserNetworkDirection, BrowserNetworkEventKind, BrowserNetworkFrameOptions, BrowserNetworkOperation,
    BrowserNetworkPayloadEncoding, BrowserNetworkRecord, DEFAULT_NETWORK_MAX_FILE_BYTES,
    DEFAULT_NETWORK_MAX_PAYLOAD_BYTES, MAX_NETWORK_FILE_BYTES, MAX_NETWORK_PAYLOAD_BYTES, MAX_NETWORK_URL_PATTERNS,
};
use writer::CaptureWriter;

const MAX_KNOWN_CONNECTIONS: usize = 4_096;
const MAX_RETURNED_CONNECTIONS: usize = 256;
const MAX_ERROR_BYTES: usize = 2 * 1024;
const MAX_CAPTURE_ID_BYTES: usize = 96;
const MAX_CONNECTION_ID_BYTES: usize = 4 * 1024;
const MAX_PUBLIC_URL_BYTES: usize = 16 * 1024;
const MAX_METHOD_BYTES: usize = 256;
const MAX_RESOURCE_TYPE_BYTES: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct NetworkCaptureState {
    active: Option<ActiveCapture>,
    last: Option<BrowserNetworkCapture>,
}

#[derive(Debug)]
struct ActiveCapture {
    capture_id: String,
    backend: BackendKind,
    transport: String,
    options: BrowserNetworkCaptureOptions,
    writer: CaptureWriter,
    sequence: u64,
    http_requests: HashMap<String, String>,
    connections: HashMap<String, TrackedConnection>,
}

#[derive(Debug)]
struct TrackedConnection {
    public: BrowserNetworkConnection,
}

impl NetworkCaptureState {
    pub(crate) fn start(
        &mut self,
        directory: Option<&Path>,
        capture_id: &str,
        backend: BackendKind,
        transport: &str,
        options: BrowserNetworkCaptureOptions,
    ) -> Result<BrowserNetworkCapture, BrowserControlFailure> {
        if self.active.is_some() {
            return Err(BrowserControlFailure::new(
                "capture_active",
                "a browser network capture is already active",
            ));
        }
        if capture_id.trim().is_empty()
            || capture_id.len() > MAX_CAPTURE_ID_BYTES
            || capture_id.chars().any(char::is_control)
        {
            return Err(BrowserControlFailure::new(
                "invalid_input",
                "network capture id must be a short printable value",
            ));
        }
        options
            .validate()
            .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
        let directory = directory.ok_or_else(|| {
            BrowserControlFailure::new(
                "capture_unavailable",
                "the browser host did not configure a capture directory",
            )
        })?;
        let writer = CaptureWriter::start(directory, capture_id, options.max_file_bytes)
            .map_err(|error| BrowserControlFailure::new("capture_io", error.to_string()))?;
        self.active = Some(ActiveCapture {
            capture_id: capture_id.to_string(),
            backend,
            transport: transport.to_string(),
            options,
            writer,
            sequence: 0,
            http_requests: HashMap::new(),
            connections: HashMap::new(),
        });
        self.record_empty(BrowserNetworkEventKind::CaptureStarted);
        self.status()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn status(&self) -> Result<BrowserNetworkCapture, BrowserControlFailure> {
        if let Some(active) = self.active.as_ref() {
            return Ok(active.summary(true));
        }
        self.last.clone().ok_or_else(|| {
            BrowserControlFailure::new("capture_not_started", "no browser network capture has been started")
        })
    }

    pub(crate) fn stop(&mut self) -> Result<BrowserNetworkCapture, BrowserControlFailure> {
        if self.active.is_none() {
            return self.status();
        }
        self.record_empty(BrowserNetworkEventKind::CaptureStopped);
        let mut active = self.active.take().ok_or_else(|| {
            BrowserControlFailure::new("capture_not_started", "no browser network capture has been started")
        })?;
        let finish_error = active.writer.finish().err();
        let mut summary = active.summary(false);
        if finish_error.is_some() {
            summary.writer_failed = true;
        }
        self.last = Some(summary.clone());
        if let Some(error) = finish_error {
            Err(BrowserControlFailure::new("capture_io", error.to_string()))
        } else {
            Ok(summary)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_http(
        &mut self,
        kind: BrowserNetworkEventKind,
        connection_id: Option<&str>,
        url: Option<&str>,
        method: Option<&str>,
        status: Option<u16>,
        resource_type: Option<&str>,
        payload_bytes: Option<u64>,
        error: Option<&str>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.options.include_http || connection_id.is_some_and(|id| !connection_id_is_bounded(id)) {
            return;
        }
        let effective_url = if let Some(url) = url {
            if !active.options.matches_url(url) {
                return;
            }
            let public_url = bounded_public_url(url);
            if let Some(id) = connection_id
                && (active.http_requests.len() < MAX_KNOWN_CONNECTIONS || active.http_requests.contains_key(id))
            {
                active.http_requests.insert(id.to_string(), public_url.clone());
            }
            Some(public_url)
        } else {
            let public_url = connection_id.and_then(|id| active.http_requests.get(id)).cloned();
            if public_url.is_none() && !active.options.url_patterns.is_empty() {
                return;
            }
            public_url
        };
        let mut record = active.next_record(kind);
        record.connection_id = connection_id.map(str::to_string);
        record.url = effective_url;
        record.method = method.map(|value| bounded_string(value, MAX_METHOD_BYTES).0);
        record.status = status;
        record.resource_type = resource_type.map(|value| bounded_string(value, MAX_RESOURCE_TYPE_BYTES).0);
        record.payload_bytes = payload_bytes;
        record.error = error.map(|value| bounded_string(value, MAX_ERROR_BYTES).0);
        active.writer.try_record(record);
        if matches!(
            kind,
            BrowserNetworkEventKind::HttpCompleted | BrowserNetworkEventKind::HttpFailed
        ) && let Some(id) = connection_id
        {
            active.http_requests.remove(id);
        }
    }

    pub(crate) fn record_websocket_created(&mut self, connection_id: &str, url: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.options.include_websocket
            || !connection_id_is_bounded(connection_id)
            || !active.options.matches_url(url)
        {
            return;
        }
        let record = active.next_record(BrowserNetworkEventKind::WebsocketCreated);
        let sequence = record.sequence;
        let timestamp = record.timestamp_millis;
        let public_url = bounded_public_url(url);
        if active.connections.len() < MAX_KNOWN_CONNECTIONS || active.connections.contains_key(connection_id) {
            active.connections.insert(
                connection_id.to_string(),
                TrackedConnection {
                    public: BrowserNetworkConnection {
                        connection_id: connection_id.to_string(),
                        url: Some(public_url.clone()),
                        state: BrowserNetworkConnectionState::Connecting,
                        observed_existing: false,
                        created_at_millis: timestamp,
                        opened_at_millis: None,
                        closed_at_millis: None,
                        last_error: None,
                        sent_frames: 0,
                        sent_bytes: 0,
                        received_frames: 0,
                        received_bytes: 0,
                        last_sequence: sequence,
                    },
                },
            );
        }
        let mut record = record;
        record.connection_id = Some(connection_id.to_string());
        record.url = Some(public_url);
        active.writer.try_record(record);
    }

    pub(crate) fn record_websocket_opened(&mut self, connection_id: &str, url: Option<&str>, observed_existing: bool) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.options.include_websocket || !connection_id_is_bounded(connection_id) {
            return;
        }
        if !active.connections.contains_key(connection_id) {
            match url {
                Some(url) if active.options.matches_url(url) => active.insert_connection(
                    connection_id,
                    Some(url),
                    BrowserNetworkConnectionState::Open,
                    observed_existing,
                    BrowserNetworkEventKind::WebsocketOpened,
                ),
                None if active.options.url_patterns.is_empty() => active.insert_connection(
                    connection_id,
                    None,
                    BrowserNetworkConnectionState::Open,
                    true,
                    BrowserNetworkEventKind::WebsocketOpened,
                ),
                Some(_) | None => {}
            }
            return;
        }
        let mut record = active.next_record(BrowserNetworkEventKind::WebsocketOpened);
        record.connection_id = Some(connection_id.to_string());
        if let Some(connection) = active.connections.get_mut(connection_id) {
            connection.public.state = BrowserNetworkConnectionState::Open;
            connection.public.observed_existing |= observed_existing;
            connection.public.opened_at_millis = Some(record.timestamp_millis);
            connection.public.last_sequence = record.sequence;
            record.url.clone_from(&connection.public.url);
        }
        active.writer.try_record(record);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_websocket_frame(
        &mut self,
        connection_id: &str,
        url: Option<&str>,
        direction: BrowserNetworkDirection,
        opcode: Option<u8>,
        payload: Option<&str>,
        encoding: BrowserNetworkPayloadEncoding,
        payload_bytes: u64,
        source_truncated: bool,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.options.include_websocket
            || !connection_id_is_bounded(connection_id)
            || matches!(direction, BrowserNetworkDirection::Sent) && !active.options.frames.include_sent
            || matches!(direction, BrowserNetworkDirection::Received) && !active.options.frames.include_received
        {
            return;
        }
        if !active.connections.contains_key(connection_id) {
            let Some(url) = url else {
                if !active.options.url_patterns.is_empty() {
                    return;
                }
                active.insert_existing_unknown(connection_id);
                if !active.connections.contains_key(connection_id) {
                    return;
                }
                return active.record_websocket_frame(
                    connection_id,
                    direction,
                    opcode,
                    payload,
                    encoding,
                    payload_bytes,
                    source_truncated,
                );
            };
            if !active.options.matches_url(url) {
                return;
            }
            active.insert_existing(connection_id, url);
        }
        if !active.connections.contains_key(connection_id) {
            return;
        }
        active.record_websocket_frame(
            connection_id,
            direction,
            opcode,
            payload,
            encoding,
            payload_bytes,
            source_truncated,
        );
    }

    pub(crate) fn record_websocket_terminal(
        &mut self,
        connection_id: &str,
        kind: BrowserNetworkEventKind,
        error: Option<&str>,
    ) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.options.include_websocket || !connection_id_is_bounded(connection_id) {
            return;
        }
        if !active.connections.contains_key(connection_id) {
            if !active.options.url_patterns.is_empty() {
                return;
            }
            active.insert_connection(
                connection_id,
                None,
                BrowserNetworkConnectionState::Open,
                true,
                BrowserNetworkEventKind::WebsocketOpened,
            );
            if !active.connections.contains_key(connection_id) {
                return;
            }
        }
        let mut record = active.next_record(kind);
        record.connection_id = Some(connection_id.to_string());
        record.error = error.map(|value| bounded_string(value, MAX_ERROR_BYTES).0);
        if let Some(connection) = active.connections.get_mut(connection_id) {
            connection.public.state = if kind == BrowserNetworkEventKind::WebsocketClosed {
                BrowserNetworkConnectionState::Closed
            } else {
                BrowserNetworkConnectionState::Error
            };
            connection.public.closed_at_millis = Some(record.timestamp_millis);
            if kind == BrowserNetworkEventKind::WebsocketError {
                connection.public.last_error.clone_from(&record.error);
            }
            connection.public.last_sequence = record.sequence;
        }
        active.writer.try_record(record);
    }

    fn record_empty(&mut self, kind: BrowserNetworkEventKind) {
        if let Some(active) = self.active.as_mut() {
            let record = active.next_record(kind);
            active.writer.try_record(record);
        }
    }
}

impl Drop for NetworkCaptureState {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl ActiveCapture {
    #[allow(clippy::too_many_arguments)]
    fn record_websocket_frame(
        &mut self,
        connection_id: &str,
        direction: BrowserNetworkDirection,
        opcode: Option<u8>,
        payload: Option<&str>,
        encoding: BrowserNetworkPayloadEncoding,
        payload_bytes: u64,
        source_truncated: bool,
    ) {
        let kind = match direction {
            BrowserNetworkDirection::Sent => BrowserNetworkEventKind::WebsocketFrameSent,
            BrowserNetworkDirection::Received => BrowserNetworkEventKind::WebsocketFrameReceived,
        };
        let mut record = self.next_record(kind);
        record.connection_id = Some(connection_id.to_string());
        record.direction = Some(direction);
        record.opcode = opcode;
        record.payload_encoding = Some(encoding);
        record.payload_bytes = Some(payload_bytes);
        if let Some(payload) = payload {
            let limit = payload_string_limit(self.options.max_payload_bytes, encoding);
            let (payload, bounded) = bounded_string(payload, limit);
            record.payload = Some(payload);
            record.truncated = bounded || source_truncated;
            if record.truncated {
                self.writer.note_truncated();
            }
        }
        if let Some(connection) = self.connections.get_mut(connection_id) {
            connection.public.state = BrowserNetworkConnectionState::Open;
            connection
                .public
                .opened_at_millis
                .get_or_insert(record.timestamp_millis);
            connection.public.last_sequence = record.sequence;
            match direction {
                BrowserNetworkDirection::Sent => {
                    connection.public.sent_frames = connection.public.sent_frames.saturating_add(1);
                    connection.public.sent_bytes = connection.public.sent_bytes.saturating_add(payload_bytes);
                }
                BrowserNetworkDirection::Received => {
                    connection.public.received_frames = connection.public.received_frames.saturating_add(1);
                    connection.public.received_bytes = connection.public.received_bytes.saturating_add(payload_bytes);
                }
            }
        }
        self.writer.try_record(record);
    }

    fn next_record(&mut self, kind: BrowserNetworkEventKind) -> BrowserNetworkRecord {
        self.sequence = self.sequence.saturating_add(1);
        BrowserNetworkRecord::empty(&self.capture_id, self.sequence, self.backend, kind)
    }

    fn insert_existing_unknown(&mut self, connection_id: &str) {
        self.insert_connection(
            connection_id,
            None,
            BrowserNetworkConnectionState::Open,
            true,
            BrowserNetworkEventKind::WebsocketOpened,
        );
    }

    fn insert_existing(&mut self, connection_id: &str, url: &str) {
        self.insert_connection(
            connection_id,
            Some(url),
            BrowserNetworkConnectionState::Open,
            true,
            BrowserNetworkEventKind::WebsocketOpened,
        );
    }

    fn insert_connection(
        &mut self,
        connection_id: &str,
        url: Option<&str>,
        state: BrowserNetworkConnectionState,
        observed_existing: bool,
        kind: BrowserNetworkEventKind,
    ) {
        if self.connections.len() >= MAX_KNOWN_CONNECTIONS {
            return;
        }
        let record = self.next_record(kind);
        let public_url = url.map(bounded_public_url);
        self.connections.insert(
            connection_id.to_string(),
            TrackedConnection {
                public: BrowserNetworkConnection {
                    connection_id: connection_id.to_string(),
                    url: public_url.clone(),
                    state,
                    observed_existing,
                    created_at_millis: record.timestamp_millis,
                    opened_at_millis: (state == BrowserNetworkConnectionState::Open).then_some(record.timestamp_millis),
                    closed_at_millis: None,
                    last_error: None,
                    sent_frames: 0,
                    sent_bytes: 0,
                    received_frames: 0,
                    received_bytes: 0,
                    last_sequence: record.sequence,
                },
            },
        );
        let mut record = record;
        record.connection_id = Some(connection_id.to_string());
        record.url = public_url;
        self.writer.try_record(record);
    }

    fn summary(&self, active: bool) -> BrowserNetworkCapture {
        let metrics = self.writer.snapshot();
        let mut connections = self
            .connections
            .values()
            .map(|connection| connection.public.clone())
            .collect::<Vec<_>>();
        connections.sort_by_key(|connection| std::cmp::Reverse(connection.last_sequence));
        let connections_truncated = connections.len().saturating_sub(MAX_RETURNED_CONNECTIONS);
        connections.truncate(MAX_RETURNED_CONNECTIONS);
        BrowserNetworkCapture {
            capture_id: self.capture_id.clone(),
            path: self.writer.path().to_string_lossy().into_owned(),
            active,
            transport: self.transport.clone(),
            records_enqueued: metrics.enqueued,
            records_written: metrics.written,
            bytes_written: metrics.bytes,
            records_dropped: metrics.dropped,
            payloads_truncated: metrics.truncated,
            file_limit_reached: metrics.file_limit_reached,
            writer_failed: metrics.writer_failed,
            known_connections: connections,
            connections_truncated: u64::try_from(connections_truncated).unwrap_or(u64::MAX),
        }
    }
}

fn bounded_string(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (value[..boundary].to_string(), true)
}

fn bounded_public_url(url: &str) -> String {
    bounded_string(&crate::audit::redact_url(url), MAX_PUBLIC_URL_BYTES).0
}

fn connection_id_is_bounded(connection_id: &str) -> bool {
    !connection_id.is_empty() && connection_id.len() <= MAX_CONNECTION_ID_BYTES
}

fn payload_string_limit(max_payload_bytes: u32, encoding: BrowserNetworkPayloadEncoding) -> usize {
    let bytes = usize::try_from(max_payload_bytes).unwrap_or(usize::MAX);
    match encoding {
        BrowserNetworkPayloadEncoding::Text => bytes,
        BrowserNetworkPayloadEncoding::Base64 => bytes.saturating_add(2) / 3 * 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_bounded_and_require_a_capture_surface() {
        let mut options = BrowserNetworkCaptureOptions::default();
        assert!(options.validate().is_ok());
        options.include_websocket = false;
        assert!(options.validate().is_err());
        options.include_http = true;
        assert!(options.validate().is_ok());
        options.url_patterns = vec!["x".repeat(MAX_NETWORK_PATTERN_BYTES + 1)];
        assert!(options.validate().is_err());
    }

    #[test]
    fn capture_ids_are_bounded_before_becoming_file_names() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let mut capture = NetworkCaptureState::default();

        let error = capture
            .start(
                Some(root.path()),
                &"x".repeat(MAX_CAPTURE_ID_BYTES + 1),
                BackendKind::ChromiumCdp,
                "cdp",
                BrowserNetworkCaptureOptions::default(),
            )
            .expect_err("oversized capture id must fail");

        assert_eq!(error.code, "invalid_input");
        assert_eq!(root.path().read_dir().map_or(0, Iterator::count), 0);
    }

    #[test]
    fn http_metadata_is_bounded_redacted_and_keeps_filtered_completion() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let mut capture = NetworkCaptureState::default();
        capture
            .start(
                Some(root.path()),
                "http-bounds",
                BackendKind::FirefoxBidi,
                "webdriver_bidi",
                BrowserNetworkCaptureOptions {
                    include_http: true,
                    include_websocket: false,
                    url_patterns: vec!["FILTER_SECRET".to_string()],
                    ..BrowserNetworkCaptureOptions::default()
                },
            )
            .unwrap_or_else(|error| panic!("capture start failed: {}", error.message));
        let url = format!(
            "https://example.test/{}?token=FILTER_SECRET",
            "x".repeat(MAX_PUBLIC_URL_BYTES + 64)
        );
        capture.record_http(
            BrowserNetworkEventKind::HttpRequest,
            Some("request-1"),
            Some(&url),
            Some(&"M".repeat(MAX_METHOD_BYTES + 1)),
            None,
            Some(&"R".repeat(MAX_RESOURCE_TYPE_BYTES + 1)),
            None,
            None,
        );
        capture.record_http(
            BrowserNetworkEventKind::HttpCompleted,
            Some("request-1"),
            None,
            None,
            None,
            None,
            Some(123),
            None,
        );
        let summary = capture
            .stop()
            .unwrap_or_else(|error| panic!("capture stop failed: {}", error.message));
        let records = std::fs::read_to_string(summary.path)
            .unwrap_or_else(|error| panic!("capture read failed: {error}"))
            .lines()
            .map(|line| {
                serde_json::from_str::<BrowserNetworkRecord>(line)
                    .unwrap_or_else(|error| panic!("capture record decode failed: {error}"))
            })
            .collect::<Vec<_>>();

        let request = records
            .iter()
            .find(|record| record.kind == BrowserNetworkEventKind::HttpRequest)
            .unwrap_or_else(|| panic!("HTTP request record missing"));
        assert_eq!(request.url.as_ref().map(String::len), Some(MAX_PUBLIC_URL_BYTES));
        assert_eq!(request.method.as_ref().map(String::len), Some(MAX_METHOD_BYTES));
        assert_eq!(
            request.resource_type.as_ref().map(String::len),
            Some(MAX_RESOURCE_TYPE_BYTES)
        );
        assert!(!request.url.as_deref().unwrap_or_default().contains("FILTER_SECRET"));
        assert!(
            records
                .iter()
                .any(|record| record.kind == BrowserNetworkEventKind::HttpCompleted)
        );
    }

    #[test]
    fn websocket_lifecycle_is_bounded_and_tracks_existing_connections() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let mut capture = NetworkCaptureState::default();
        capture
            .start(
                Some(root.path()),
                "capture",
                BackendKind::ChromiumCdp,
                "cdp",
                BrowserNetworkCaptureOptions {
                    max_payload_bytes: 4,
                    ..BrowserNetworkCaptureOptions::default()
                },
            )
            .unwrap_or_else(|error| panic!("capture start failed: {}", error.message));
        capture.record_websocket_frame(
            "existing",
            None,
            BrowserNetworkDirection::Received,
            Some(1),
            Some("abcdef"),
            BrowserNetworkPayloadEncoding::Text,
            6,
            false,
        );
        capture.record_websocket_terminal("existing", BrowserNetworkEventKind::WebsocketClosed, None);
        let summary = capture
            .stop()
            .unwrap_or_else(|error| panic!("capture stop failed: {}", error.message));

        assert!(!summary.active);
        assert_eq!(summary.known_connections.len(), 1);
        assert!(summary.known_connections[0].observed_existing);
        assert_eq!(
            summary.known_connections[0].state,
            BrowserNetworkConnectionState::Closed
        );
        assert_eq!(summary.payloads_truncated, 1);
        let encoded =
            std::fs::read_to_string(&summary.path).unwrap_or_else(|error| panic!("capture read failed: {error}"));
        assert!(encoded.contains("websocket_frame_received"));
        assert!(!encoded.contains("abcdef"));
    }

    #[test]
    fn dropping_an_active_capture_flushes_a_stop_marker() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = {
            let mut capture = NetworkCaptureState::default();
            capture
                .start(
                    Some(root.path()),
                    "active-close",
                    BackendKind::ChromiumCdp,
                    "cdp",
                    BrowserNetworkCaptureOptions::default(),
                )
                .unwrap_or_else(|error| panic!("capture start failed: {}", error.message))
                .path
        };

        let encoded =
            std::fs::read_to_string(path).unwrap_or_else(|error| panic!("capture read failed after drop: {error}"));
        let last = encoded.lines().last().unwrap_or_default();
        assert!(last.contains("capture_stopped"));
    }

    #[test]
    fn an_untracked_frame_is_dropped_when_the_connection_table_is_full() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let mut capture = NetworkCaptureState::default();
        capture
            .start(
                Some(root.path()),
                "full-table",
                BackendKind::ChromiumCdp,
                "cdp",
                BrowserNetworkCaptureOptions::default(),
            )
            .unwrap_or_else(|error| panic!("capture start failed: {}", error.message));
        for index in 0..MAX_KNOWN_CONNECTIONS {
            capture.record_websocket_created(&format!("socket-{index}"), "wss://example.test/stream");
        }

        capture.record_websocket_frame(
            "untracked",
            None,
            BrowserNetworkDirection::Received,
            Some(1),
            Some("frame"),
            BrowserNetworkPayloadEncoding::Text,
            5,
            false,
        );
        let summary = capture
            .stop()
            .unwrap_or_else(|error| panic!("capture stop failed: {}", error.message));

        assert_eq!(summary.known_connections.len(), MAX_RETURNED_CONNECTIONS);
        assert_eq!(summary.connections_truncated, 3_840);
        assert!(
            summary
                .known_connections
                .iter()
                .all(|connection| connection.connection_id != "untracked")
        );
    }
}
