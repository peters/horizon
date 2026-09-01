use std::collections::HashMap;
use std::io::{BufRead, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::{
    BrowserControlAction, BrowserControlValue, BrowserNetworkCapture, BrowserNetworkEventKind, BrowserNetworkOperation,
    BrowserNetworkRecord,
};

use crate::controller::BrowserController;
use crate::model::{
    NetworkWatchCaptureState, NetworkWatchDeliveryState, NetworkWatchEventKind, NetworkWatchInput, NetworkWatchOutput,
    NetworkWatchRecord,
};

const DEFAULT_WAIT_MILLIS: u64 = 60_000;
const DEFAULT_MAX_RECORDS: u32 = 100;
const DEFAULT_MAX_PAYLOAD_BYTES: u32 = 16 * 1024;
const MAX_WAIT_MILLIS: u64 = 60_000;
const MAX_RECORDS: u32 = 250;
const MAX_URL_PATTERNS: usize = 32;
const MAX_URL_PATTERN_BYTES: usize = 2 * 1024;
const MAX_RETURNED_PAYLOAD_BYTES: u32 = 64 * 1024;
const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CACHE_ENTRIES: usize = 64;
const MAX_CONNECTION_URLS: usize = 1_024;
const MAX_CONNECTION_ID_BYTES: usize = 512;
const MAX_CONNECTION_URL_BYTES: usize = 4 * 1024;
const OWNERSHIP_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Default)]
pub(crate) struct NetworkWatchState {
    cache: Arc<Mutex<WatchCache>>,
}

#[derive(Debug, Default)]
struct WatchCache {
    entries: HashMap<String, CachedCursor>,
}

#[derive(Clone, Debug)]
struct CachedCursor {
    sequence: u64,
    offset: u64,
    connection_urls: HashMap<String, String>,
}

#[derive(Debug)]
struct ValidatedWatch {
    panel_id: String,
    expected_capture_id: Option<String>,
    after_sequence: u64,
    wait: Duration,
    max_records: usize,
    url_patterns: Vec<String>,
    event_kinds: Vec<NetworkWatchEventKind>,
    include_payload: bool,
    max_payload_bytes: usize,
    timeout_millis: Option<u64>,
}

#[derive(Debug, Default)]
struct ScanResult {
    records: Vec<NetworkWatchRecord>,
    next_sequence: u64,
    last_seen_sequence: u64,
    next_offset: u64,
    sequence_gaps: u64,
    malformed_records: u64,
    connection_urls_truncated: u64,
    returned_payloads_truncated: u64,
    capture_stopped: bool,
    file_reset: bool,
    connection_urls: HashMap<String, String>,
}

impl ScanResult {
    fn merge_poll(&mut self, poll: Self, cache_reset: bool) {
        self.records.extend(poll.records);
        self.next_sequence = poll.next_sequence;
        self.last_seen_sequence = self.last_seen_sequence.max(poll.last_seen_sequence);
        self.next_offset = poll.next_offset;
        self.sequence_gaps = self.sequence_gaps.saturating_add(poll.sequence_gaps);
        self.malformed_records = self.malformed_records.saturating_add(poll.malformed_records);
        self.connection_urls_truncated = self
            .connection_urls_truncated
            .saturating_add(poll.connection_urls_truncated);
        self.returned_payloads_truncated = self
            .returned_payloads_truncated
            .saturating_add(poll.returned_payloads_truncated);
        self.capture_stopped |= poll.capture_stopped;
        self.file_reset |= cache_reset || poll.file_reset;
    }
}

impl NetworkWatchState {
    pub(crate) async fn watch(
        &self,
        controller: &BrowserController,
        input: NetworkWatchInput,
    ) -> Result<NetworkWatchOutput, String> {
        let request = ValidatedWatch::new(input)?;
        let (mut action_id, mut capture) = capture_status(controller, &request).await?;
        let mut capture_changed = request
            .expected_capture_id
            .as_deref()
            .is_some_and(|expected| expected != capture.capture_id);
        let mut next_sequence = if capture_changed { 0 } else { request.after_sequence };
        let path = PathBuf::from(&capture.path);
        let watched_capture_id = capture.capture_id.clone();
        let cache_key = format!("{}\0{}", request.panel_id, capture.capture_id);
        let started = Instant::now();
        let mut last_heartbeat = started;
        let mut aggregate = ScanResult {
            next_sequence,
            ..ScanResult::default()
        };

        loop {
            let (start_offset, cache_reset, connection_urls) = self.start_cursor(&cache_key, next_sequence, &path);
            let scan_request = ScanRequest {
                path: path.clone(),
                capture_id: capture.capture_id.clone(),
                after_sequence: next_sequence,
                start_offset,
                max_records: request.max_records,
                url_patterns: request.url_patterns.clone(),
                event_kinds: request.event_kinds.clone(),
                include_payload: request.include_payload,
                max_payload_bytes: request.max_payload_bytes,
                connection_urls,
            };
            let scan = tokio::task::spawn_blocking(move || scan_records(&scan_request))
                .await
                .map_err(|error| format!("browser network watch reader failed: {error}"))?
                .map_err(|error| format!("could not read browser network capture: {error}"))?;
            next_sequence = scan.next_sequence;
            self.store_cursor(
                &cache_key,
                scan.last_seen_sequence,
                scan.next_offset,
                scan.connection_urls.clone(),
            );
            aggregate.merge_poll(scan, cache_reset);

            let terminal = !aggregate.records.is_empty()
                || aggregate.capture_stopped
                || !capture.active
                || capture.file_limit_reached
                || capture.writer_failed;
            if terminal {
                break;
            }
            if started.elapsed() >= request.wait {
                let (refreshed_action_id, refreshed_capture) = capture_status(controller, &request).await?;
                action_id = refreshed_action_id;
                if refreshed_capture.capture_id != watched_capture_id {
                    capture_changed = true;
                    next_sequence = 0;
                }
                capture = refreshed_capture;
                break;
            }
            if last_heartbeat.elapsed() >= OWNERSHIP_HEARTBEAT_INTERVAL {
                controller
                    .refresh_claim(&request.panel_id)
                    .map_err(|error| error.to_string())?;
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(POLL_INTERVAL.min(request.wait.saturating_sub(started.elapsed()))).await;
        }

        // A scan can complete between heartbeats; re-check ownership and
        // workspace membership under the manifest lock before disclosing
        // anything captured after the host may have moved the panel.
        controller
            .refresh_claim(&request.panel_id)
            .map_err(|error| error.to_string())?;

        let timed_out = started.elapsed() >= request.wait
            && aggregate.records.is_empty()
            && !aggregate.capture_stopped
            && capture.active;
        Ok(NetworkWatchOutput {
            panel_id: request.panel_id,
            action_id,
            capture_id: capture.capture_id,
            records: aggregate.records,
            next_sequence,
            delivery: NetworkWatchDeliveryState {
                timed_out,
                capture_changed,
                file_reset: aggregate.file_reset,
            },
            sequence_gaps: aggregate.sequence_gaps,
            malformed_records: aggregate.malformed_records,
            connection_urls_truncated: aggregate.connection_urls_truncated,
            records_dropped: capture.records_dropped,
            payloads_truncated: capture.payloads_truncated,
            returned_payloads_truncated: aggregate.returned_payloads_truncated,
            capture_state: NetworkWatchCaptureState {
                capture_active: capture.active && !aggregate.capture_stopped,
                file_limit_reached: capture.file_limit_reached,
                writer_failed: capture.writer_failed,
            },
        })
    }

    fn start_cursor(&self, key: &str, after_sequence: u64, path: &Path) -> (u64, bool, HashMap<String, String>) {
        let file_len = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
        let cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(cursor) = cache.entries.get(key) else {
            return (0, false, HashMap::new());
        };
        if cursor.sequence <= after_sequence && cursor.offset <= file_len {
            (cursor.offset, false, cursor.connection_urls.clone())
        } else {
            (0, cursor.offset > file_len, HashMap::new())
        }
    }

    fn store_cursor(&self, key: &str, sequence: u64, offset: u64, connection_urls: HashMap<String, String>) {
        let mut cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.entries.len() >= MAX_CACHE_ENTRIES
            && !cache.entries.contains_key(key)
            && let Some(oldest) = cache.entries.keys().next().cloned()
        {
            cache.entries.remove(&oldest);
        }
        cache.entries.insert(
            key.to_string(),
            CachedCursor {
                sequence,
                offset,
                connection_urls,
            },
        );
    }
}

impl ValidatedWatch {
    fn new(input: NetworkWatchInput) -> Result<Self, String> {
        let include_payload = input.include_payload.unwrap_or(false);
        if !include_payload && input.max_payload_bytes.is_some() {
            return Err("max_payload_bytes requires include_payload=true".to_string());
        }
        let max_payload_bytes = input.max_payload_bytes.unwrap_or(DEFAULT_MAX_PAYLOAD_BYTES);
        if !(1..=MAX_RETURNED_PAYLOAD_BYTES).contains(&max_payload_bytes) {
            return Err(format!(
                "max_payload_bytes must be between 1 and {MAX_RETURNED_PAYLOAD_BYTES}"
            ));
        }
        let url_patterns = input.url_patterns.unwrap_or_default();
        if url_patterns.len() > MAX_URL_PATTERNS
            || url_patterns.iter().any(|pattern| {
                pattern.trim().is_empty()
                    || pattern.len() > MAX_URL_PATTERN_BYTES
                    || pattern.chars().any(char::is_control)
            })
        {
            return Err("URL patterns must be a bounded list of short printable substrings".to_string());
        }
        let event_kinds = input.event_kinds.unwrap_or_default();
        if event_kinds.len() > 32 {
            return Err("event_kinds accepts at most 32 entries".to_string());
        }
        Ok(Self {
            panel_id: input.panel_id,
            expected_capture_id: input.capture_id,
            after_sequence: input.after_sequence.unwrap_or(0),
            wait: Duration::from_millis(
                input
                    .wait_millis
                    .unwrap_or(DEFAULT_WAIT_MILLIS)
                    .clamp(1, MAX_WAIT_MILLIS),
            ),
            max_records: usize::try_from(input.max_records.unwrap_or(DEFAULT_MAX_RECORDS).clamp(1, MAX_RECORDS))
                .unwrap_or(usize::MAX),
            url_patterns,
            event_kinds,
            include_payload,
            max_payload_bytes: usize::try_from(max_payload_bytes).unwrap_or(usize::MAX),
            timeout_millis: input.timeout_millis,
        })
    }
}

async fn capture_status(
    controller: &BrowserController,
    request: &ValidatedWatch,
) -> Result<(String, BrowserNetworkCapture), String> {
    let receipt = controller
        .execute(
            &request.panel_id,
            BrowserControlAction::Network {
                operation: BrowserNetworkOperation::Status,
                options: None,
            },
            request.timeout_millis,
        )
        .await
        .map_err(|error| error.to_string())?;
    let BrowserControlValue::Network { capture } = receipt.value else {
        return Err("browser returned an unexpected network status result".to_string());
    };
    Ok((receipt.action_id, capture))
}

#[derive(Debug)]
struct ScanRequest {
    path: PathBuf,
    capture_id: String,
    after_sequence: u64,
    start_offset: u64,
    max_records: usize,
    url_patterns: Vec<String>,
    event_kinds: Vec<NetworkWatchEventKind>,
    include_payload: bool,
    max_payload_bytes: usize,
    connection_urls: HashMap<String, String>,
}

fn scan_records(request: &ScanRequest) -> std::io::Result<ScanResult> {
    let Some((mut reader, start_offset, file_reset)) = open_capture(request)? else {
        return Ok(ScanResult {
            next_sequence: request.after_sequence,
            ..ScanResult::default()
        });
    };
    let mut result = ScanResult {
        next_sequence: request.after_sequence,
        last_seen_sequence: request.after_sequence,
        next_offset: start_offset,
        file_reset,
        connection_urls: request.connection_urls.clone(),
        ..ScanResult::default()
    };
    let mut expected_sequence = request.after_sequence.saturating_add(1);
    let mut remaining_payload_bytes = request.max_payload_bytes;
    let mut encoded = Vec::new();
    loop {
        encoded.clear();
        let line_start = result.next_offset;
        let read = reader.read_until(b'\n', &mut encoded)?;
        if read == 0 {
            break;
        }
        if encoded.last() != Some(&b'\n') {
            result.next_offset = line_start;
            break;
        }
        result.next_offset = result
            .next_offset
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if encoded.len() > MAX_RECORD_BYTES {
            result.malformed_records = result.malformed_records.saturating_add(1);
            continue;
        }
        let mut record = match serde_json::from_slice::<BrowserNetworkRecord>(&encoded) {
            Ok(record) if record.capture_id == request.capture_id => record,
            Ok(_) => continue,
            Err(_) => {
                result.malformed_records = result.malformed_records.saturating_add(1);
                continue;
            }
        };
        if websocket_event(record.kind) {
            if let (Some(connection_id), Some(url)) = (record.connection_id.as_ref(), record.url.as_ref()) {
                if connection_id.len() <= MAX_CONNECTION_ID_BYTES && url.len() <= MAX_CONNECTION_URL_BYTES {
                    if result.connection_urls.len() >= MAX_CONNECTION_URLS
                        && !result.connection_urls.contains_key(connection_id)
                        && let Some(evicted) = result.connection_urls.keys().next().cloned()
                    {
                        result.connection_urls.remove(&evicted);
                        result.connection_urls_truncated = result.connection_urls_truncated.saturating_add(1);
                    }
                    result.connection_urls.insert(connection_id.clone(), url.clone());
                } else {
                    result.connection_urls_truncated = result.connection_urls_truncated.saturating_add(1);
                }
            } else if let Some(url) = record
                .connection_id
                .as_ref()
                .and_then(|connection_id| result.connection_urls.get(connection_id))
            {
                record.url = Some(url.clone());
            }
        }
        result.last_seen_sequence = result.last_seen_sequence.max(record.sequence);
        if record.sequence <= request.after_sequence {
            continue;
        }
        if record.sequence > expected_sequence {
            result.sequence_gaps = result
                .sequence_gaps
                .saturating_add(record.sequence.saturating_sub(expected_sequence));
        }
        expected_sequence = expected_sequence.max(record.sequence.saturating_add(1));
        result.next_sequence = result.next_sequence.max(record.sequence);
        result.capture_stopped |= record.kind == BrowserNetworkEventKind::CaptureStopped;
        if !record_matches(request, &record) {
            continue;
        }
        let (output, watcher_truncated) = bounded_record(request, record, &mut remaining_payload_bytes);
        result.returned_payloads_truncated = result
            .returned_payloads_truncated
            .saturating_add(u64::from(watcher_truncated));
        result.records.push(output);
        if result.records.len() >= request.max_records {
            break;
        }
    }
    Ok(result)
}

fn bounded_record(
    request: &ScanRequest,
    record: BrowserNetworkRecord,
    remaining_payload_bytes: &mut usize,
) -> (NetworkWatchRecord, bool) {
    let watcher_truncated = request.include_payload
        && record
            .payload
            .as_ref()
            .is_some_and(|payload| payload.len() > *remaining_payload_bytes);
    let omit_payload = watcher_truncated && request.max_records > 1;
    let budget = if omit_payload { 0 } else { *remaining_payload_bytes };
    let mut output = NetworkWatchRecord::from_record(record, request.include_payload, budget);
    if omit_payload {
        output.payload = None;
        output.truncated = true;
    }
    *remaining_payload_bytes = remaining_payload_bytes.saturating_sub(output.payload.as_ref().map_or(0, String::len));
    (output, watcher_truncated)
}

fn open_capture(request: &ScanRequest) -> std::io::Result<Option<(std::io::BufReader<std::fs::File>, u64, bool)>> {
    let mut file = match std::fs::File::open(&request.path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let file_reset = request.start_offset > file.metadata()?.len();
    let start_offset = if file_reset { 0 } else { request.start_offset };
    file.seek(std::io::SeekFrom::Start(start_offset))?;
    Ok(Some((std::io::BufReader::new(file), start_offset, file_reset)))
}

fn record_matches(request: &ScanRequest, record: &BrowserNetworkRecord) -> bool {
    let kind_matches =
        request.event_kinds.is_empty() || request.event_kinds.iter().any(|kind| kind.matches(record.kind));
    let url_matches = request.url_patterns.is_empty()
        || record
            .url
            .as_deref()
            .is_some_and(|url| request.url_patterns.iter().any(|pattern| url.contains(pattern)));
    kind_matches && url_matches
}

const fn websocket_event(kind: BrowserNetworkEventKind) -> bool {
    matches!(
        kind,
        BrowserNetworkEventKind::WebsocketCreated
            | BrowserNetworkEventKind::WebsocketOpened
            | BrowserNetworkEventKind::WebsocketFrameSent
            | BrowserNetworkEventKind::WebsocketFrameReceived
            | BrowserNetworkEventKind::WebsocketError
            | BrowserNetworkEventKind::WebsocketClosed
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use horizon_browser::{BackendKind, BrowserNetworkPayloadEncoding};

    use super::*;

    fn record(capture_id: &str, sequence: u64, kind: BrowserNetworkEventKind, url: &str) -> BrowserNetworkRecord {
        let mut record = BrowserNetworkRecord {
            schema_version: 1,
            capture_id: capture_id.to_string(),
            sequence,
            timestamp_millis: 42,
            backend: BackendKind::FirefoxBidi,
            kind,
            connection_id: None,
            url: Some(url.to_string()),
            method: Some("GET".to_string()),
            status: Some(200),
            resource_type: None,
            direction: None,
            opcode: None,
            payload: None,
            payload_encoding: None,
            payload_bytes: None,
            truncated: false,
            error: None,
        };
        if kind == BrowserNetworkEventKind::HttpResponseBody {
            record.payload = Some("quoted-price-payload".to_string());
            record.payload_encoding = Some(BrowserNetworkPayloadEncoding::Text);
            record.payload_bytes = Some(20);
        }
        record
    }

    fn request(path: PathBuf, after_sequence: u64) -> ScanRequest {
        ScanRequest {
            path,
            capture_id: "capture-1".to_string(),
            after_sequence,
            start_offset: 0,
            max_records: 100,
            url_patterns: vec!["NO0010096985-XOSL".to_string()],
            event_kinds: vec![NetworkWatchEventKind::HttpResponseBody],
            include_payload: false,
            max_payload_bytes: 16,
            connection_urls: HashMap::new(),
        }
    }

    fn watch_input() -> NetworkWatchInput {
        NetworkWatchInput {
            panel_id: "panel-1".to_string(),
            capture_id: None,
            after_sequence: None,
            wait_millis: None,
            max_records: None,
            url_patterns: None,
            event_kinds: None,
            include_payload: None,
            max_payload_bytes: None,
            timeout_millis: None,
        }
    }

    fn write_record(file: &mut std::fs::File, record: &BrowserNetworkRecord) {
        serde_json::to_writer(&mut *file, record).unwrap_or_else(|error| panic!("encode failed: {error}"));
        file.write_all(b"\n")
            .unwrap_or_else(|error| panic!("write failed: {error}"));
    }

    #[test]
    fn scan_filters_resumes_without_duplicates_and_excludes_payloads() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let records = [
            record(
                "capture-1",
                1,
                BrowserNetworkEventKind::HttpRequest,
                "https://e24.no/other",
            ),
            record(
                "capture-1",
                2,
                BrowserNetworkEventKind::HttpResponseBody,
                "https://e24.no/NO0010096985-XOSL",
            ),
        ];
        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        for record in records {
            write_record(&mut file, &record);
        }
        let first = scan_records(&request(path.clone(), 0)).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert_eq!(first.records.len(), 1);
        assert_eq!(first.next_sequence, 2);
        assert!(first.records[0].payload.is_none());

        let second = scan_records(&request(path, first.next_sequence))
            .unwrap_or_else(|error| panic!("resume scan failed: {error}"));
        assert!(second.records.is_empty());
        assert_eq!(second.next_sequence, 2);
    }

    #[test]
    fn scan_retains_partial_final_line_for_a_later_poll() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let event = record(
            "capture-1",
            1,
            BrowserNetworkEventKind::HttpResponseBody,
            "https://e24.no/NO0010096985-XOSL",
        );
        let encoded = serde_json::to_vec(&event).unwrap_or_else(|error| panic!("encode failed: {error}"));
        std::fs::write(&path, &encoded[..encoded.len() / 2])
            .unwrap_or_else(|error| panic!("partial write failed: {error}"));
        let first = scan_records(&request(path.clone(), 0)).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert!(first.records.is_empty());
        assert_eq!(first.next_offset, 0);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap_or_else(|error| panic!("open failed: {error}"));
        file.write_all(&encoded[encoded.len() / 2..])
            .unwrap_or_else(|error| panic!("append failed: {error}"));
        file.write_all(b"\n")
            .unwrap_or_else(|error| panic!("newline failed: {error}"));
        let second = scan_records(&request(path, 0)).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert_eq!(second.records.len(), 1);
    }

    #[test]
    fn scan_reports_malformed_lines_gaps_stops_and_file_reset() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        write_record(
            &mut file,
            &record(
                "capture-1",
                1,
                BrowserNetworkEventKind::HttpRequest,
                "https://e24.no/other",
            ),
        );
        file.write_all(b"{not-json}\n")
            .unwrap_or_else(|error| panic!("malformed write failed: {error}"));
        write_record(
            &mut file,
            &record(
                "capture-1",
                3,
                BrowserNetworkEventKind::CaptureStopped,
                "https://e24.no/other",
            ),
        );
        let mut scan_request = request(path.clone(), 0);
        scan_request.start_offset = file
            .metadata()
            .unwrap_or_else(|error| panic!("metadata failed: {error}"))
            .len()
            .saturating_add(1);

        let result = scan_records(&scan_request).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert!(result.records.is_empty());
        assert!(result.capture_stopped);
        assert!(result.file_reset);
        assert_eq!(result.next_sequence, 3);
        assert_eq!(result.sequence_gaps, 1);
        assert_eq!(result.malformed_records, 1);
    }

    #[test]
    fn scan_applies_an_additional_return_payload_limit() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        write_record(
            &mut file,
            &record(
                "capture-1",
                1,
                BrowserNetworkEventKind::HttpResponseBody,
                "https://e24.no/NO0010096985-XOSL",
            ),
        );
        let mut scan_request = request(path, 0);
        scan_request.include_payload = true;
        scan_request.max_payload_bytes = 5;
        scan_request.max_records = 1;

        let result = scan_records(&scan_request).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].payload.as_deref(), Some("quote"));
        assert!(result.records[0].truncated);
        assert_eq!(result.returned_payloads_truncated, 1);
    }

    #[test]
    fn scan_preserves_small_payloads_within_the_aggregate_budget() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let mut large = record(
            "capture-1",
            1,
            BrowserNetworkEventKind::HttpResponseBody,
            "https://e24.no/NO0010096985-XOSL",
        );
        let mut small = record(
            "capture-1",
            2,
            BrowserNetworkEventKind::HttpResponseBody,
            "https://e24.no/NO0010096985-XOSL",
        );
        large.payload = Some("oversized-payload".to_string());
        small.payload = Some("ok".to_string());
        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        write_record(&mut file, &large);
        write_record(&mut file, &small);
        let mut scan_request = request(path, 0);
        scan_request.include_payload = true;
        scan_request.max_payload_bytes = 5;

        let result = scan_records(&scan_request).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert_eq!(result.records.len(), 2);
        assert!(result.records[0].payload.is_none());
        assert!(result.records[0].truncated);
        assert_eq!(result.records[1].payload.as_deref(), Some("ok"));
        assert!(!result.records[1].truncated);
        assert_eq!(result.returned_payloads_truncated, 1);
    }

    #[test]
    fn scan_resolves_frame_urls_from_their_connection_lifecycle() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let mut frame = record("capture-1", 2, BrowserNetworkEventKind::WebsocketFrameReceived, "");
        frame.connection_id = Some("socket-1".to_string());
        frame.url = None;
        let mut created = record(
            "capture-1",
            1,
            BrowserNetworkEventKind::WebsocketCreated,
            "wss://example.test/NO0010096985-XOSL",
        );
        created.connection_id = Some("socket-1".to_string());

        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        write_record(&mut file, &created);
        write_record(&mut file, &frame);
        let mut scan_request = request(path, 0);
        scan_request.event_kinds = vec![NetworkWatchEventKind::WebsocketFrameReceived];

        let result = scan_records(&scan_request).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert_eq!(result.records.len(), 1);
        assert_eq!(
            result.records[0].url.as_deref(),
            Some("wss://example.test/NO0010096985-XOSL")
        );
    }

    #[test]
    fn scan_reports_connection_url_cache_bounds() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("capture.ndjson");
        let connection_id = "x".repeat(MAX_CONNECTION_ID_BYTES + 1);
        let mut created = record(
            "capture-1",
            1,
            BrowserNetworkEventKind::WebsocketCreated,
            "wss://example.test/NO0010096985-XOSL",
        );
        created.connection_id = Some(connection_id.clone());
        let mut frame = record("capture-1", 2, BrowserNetworkEventKind::WebsocketFrameReceived, "");
        frame.connection_id = Some(connection_id);
        frame.url = None;
        let mut file = std::fs::File::create(&path).unwrap_or_else(|error| panic!("create failed: {error}"));
        write_record(&mut file, &created);
        write_record(&mut file, &frame);
        let mut scan_request = request(path, 0);
        scan_request.event_kinds = vec![NetworkWatchEventKind::WebsocketFrameReceived];

        let result = scan_records(&scan_request).unwrap_or_else(|error| panic!("scan failed: {error}"));
        assert!(result.records.is_empty());
        assert_eq!(result.connection_urls_truncated, 1);
    }

    #[test]
    fn validation_bounds_wait_batches_filters_and_payloads() {
        let mut input = watch_input();
        input.wait_millis = Some(u64::MAX);
        input.max_records = Some(u32::MAX);
        let validated = ValidatedWatch::new(input).unwrap_or_else(|error| panic!("validation failed: {error}"));
        assert_eq!(validated.wait, Duration::from_millis(MAX_WAIT_MILLIS));
        assert_eq!(
            validated.max_records,
            usize::try_from(MAX_RECORDS).unwrap_or(usize::MAX)
        );

        let mut payload_without_opt_in = watch_input();
        payload_without_opt_in.max_payload_bytes = Some(1);
        assert!(ValidatedWatch::new(payload_without_opt_in).is_err());

        let mut oversized_payload = watch_input();
        oversized_payload.include_payload = Some(true);
        oversized_payload.max_payload_bytes = Some(MAX_RETURNED_PAYLOAD_BYTES + 1);
        assert!(ValidatedWatch::new(oversized_payload).is_err());

        let mut invalid_pattern = watch_input();
        invalid_pattern.url_patterns = Some(vec!["\n".to_string()]);
        assert!(ValidatedWatch::new(invalid_pattern).is_err());

        let mut excessive_patterns = watch_input();
        excessive_patterns.url_patterns = Some(vec!["quote".to_string(); MAX_URL_PATTERNS + 1]);
        assert!(ValidatedWatch::new(excessive_patterns).is_err());
    }
}
