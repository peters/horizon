//! Chromium response-body retrieval evidence.
//!
//! `Network.getResponseBody` answers `{ body: "", base64Encoded: false }`
//! both for a genuinely empty response and for a body Blink never buffered,
//! for example a `fetch()` response the page drained straight into a `Blob`.
//! The driver therefore keeps a bounded record of what each tracked response
//! declared and what Chromium transferred, and turns an unexplained empty
//! body into an explicit capture error instead of a silent zero-byte success.

use std::collections::HashMap;

use crate::BrowserNetworkPayloadEncoding;
use crate::network::decoded_base64_len;

const MAX_TRACKED_RESPONSES: usize = 4_096;
const MAX_METHOD_BYTES: usize = 32;
const MAX_MIME_TYPE_BYTES: usize = 128;
/// Chromium counts transport framing such as chunk terminators, HTTP/2 frame
/// headers, and trailers in `encodedDataLength`, so an empty body can still
/// leave a few bytes unaccounted for after the response headers.
const FRAMING_ALLOWANCE_BYTES: u64 = 64;

/// A response whose body must still be fetched through `Network.getResponseBody`.
#[derive(Debug)]
pub(super) struct PendingHttpBody {
    pub(super) request_id: String,
    pub(super) url: String,
    pub(super) evidence: Option<HttpBodyEvidence>,
}

/// What Chromium reported about one response before its body was requested.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct HttpBodyEvidence {
    method: Option<String>,
    status: Option<u16>,
    mime_type: Option<String>,
    content_length: Option<u64>,
    /// `response.encodedDataLength` from `Network.responseReceived`: the
    /// bytes transferred before any body arrived, in practice the headers.
    header_bytes: Option<u64>,
    /// Decoded body bytes Blink handed to `DevTools` via `Network.dataReceived`.
    observed_body_bytes: u64,
    /// Total transferred bytes from `Network.loadingFinished`.
    encoded_data_length: Option<u64>,
}

/// The outcome of decoding one `Network.getResponseBody` result.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CdpBodyOutcome<'a> {
    Captured {
        payload: &'a str,
        encoding: BrowserNetworkPayloadEncoding,
        payload_bytes: u64,
    },
    Unavailable(String),
}

/// Decode a `Network.getResponseBody` result, refusing to report an
/// unexplained empty body as a successfully captured zero-byte payload.
pub(super) fn decode_cdp_body<'a>(
    evidence: Option<&HttpBodyEvidence>,
    result: &'a serde_json::Value,
) -> CdpBodyOutcome<'a> {
    let Some(payload) = result.get("body").and_then(serde_json::Value::as_str) else {
        return CdpBodyOutcome::Unavailable("Chromium omitted the response body".to_string());
    };
    let base64 = result
        .get("base64Encoded")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if payload.is_empty()
        && let Some(reason) = explain_empty_body(evidence)
    {
        return CdpBodyOutcome::Unavailable(reason);
    }
    let (encoding, payload_bytes) = if base64 {
        (BrowserNetworkPayloadEncoding::Base64, decoded_base64_len(payload))
    } else {
        (
            BrowserNetworkPayloadEncoding::Text,
            u64::try_from(payload.len()).unwrap_or(u64::MAX),
        )
    };
    CdpBodyOutcome::Captured {
        payload,
        encoding,
        payload_bytes,
    }
}

/// `Some(reason)` when the evidence says the response carried a body that
/// Chromium could not hand back; `None` when an empty body is plausible.
fn explain_empty_body(evidence: Option<&HttpBodyEvidence>) -> Option<String> {
    let Some(evidence) = evidence else {
        return Some(
            "Chromium returned an empty body and Horizon retained no response metadata to confirm that the response was empty"
                .to_string(),
        );
    };
    if evidence.body_is_forbidden() || evidence.content_length == Some(0) {
        return None;
    }
    let mime = evidence
        .mime_type
        .as_deref()
        .map(|mime| format!(" ({mime})"))
        .unwrap_or_default();
    if evidence.observed_body_bytes > 0 {
        return Some(format!(
            "Chromium returned an empty body although it observed {} decoded body bytes for this response{mime}; DevTools did not retain the body",
            evidence.observed_body_bytes
        ));
    }
    if let Some(declared) = evidence.content_length {
        return Some(format!(
            "Chromium returned an empty body for a response that declared Content-Length {declared}{mime}; the page probably consumed the body as a Blob, which bypasses CDP body capture"
        ));
    }
    let unaccounted = evidence.unaccounted_encoded_bytes();
    (unaccounted > FRAMING_ALLOWANCE_BYTES).then(|| {
        format!(
            "Chromium returned an empty body for a response that transferred {unaccounted} encoded bytes beyond its headers{mime}; the page probably consumed the body as a Blob, which bypasses CDP body capture"
        )
    })
}

impl HttpBodyEvidence {
    fn body_is_forbidden(&self) -> bool {
        let status_forbids_body = self
            .status
            .is_some_and(|status| matches!(status, 100..=199 | 204 | 205 | 304));
        let head_request = self
            .method
            .as_deref()
            .is_some_and(|method| method.eq_ignore_ascii_case("HEAD"));
        status_forbids_body || head_request
    }

    fn unaccounted_encoded_bytes(&self) -> u64 {
        match (self.encoded_data_length, self.header_bytes) {
            (Some(total), Some(headers)) => total.saturating_sub(headers),
            _ => 0,
        }
    }
}

/// Bounded per-request evidence for responses whose bodies will be fetched.
#[derive(Debug, Default)]
pub(super) struct HttpBodyEvidenceTable {
    in_flight: HashMap<String, HttpBodyEvidence>,
}

impl HttpBodyEvidenceTable {
    /// Start (or, after a redirect, restart) tracking one request.
    pub(super) fn begin(&mut self, request_id: &str, method: Option<&str>) {
        if self.in_flight.len() >= MAX_TRACKED_RESPONSES && !self.in_flight.contains_key(request_id) {
            return;
        }
        let evidence = HttpBodyEvidence {
            method: method.map(|value| bounded(value, MAX_METHOD_BYTES)),
            ..HttpBodyEvidence::default()
        };
        self.in_flight.insert(request_id.to_string(), evidence);
    }

    /// Record the `response` object of `Network.responseReceived`.
    pub(super) fn note_response(&mut self, request_id: &str, response: &serde_json::Value) {
        let Some(evidence) = self.in_flight.get_mut(request_id) else {
            return;
        };
        evidence.status = response
            .get("status")
            .and_then(serde_json::Value::as_u64)
            .and_then(|status| u16::try_from(status).ok());
        evidence.mime_type = response
            .get("mimeType")
            .and_then(serde_json::Value::as_str)
            .filter(|mime| !mime.is_empty())
            .map(|mime| bounded(mime, MAX_MIME_TYPE_BYTES));
        evidence.content_length = response.get("headers").and_then(content_length_header);
        evidence.header_bytes = response.get("encodedDataLength").and_then(serde_json::Value::as_u64);
    }

    /// Record one `Network.dataReceived` chunk.
    pub(super) fn note_data(&mut self, request_id: &str, data_length: u64) {
        if let Some(evidence) = self.in_flight.get_mut(request_id) {
            evidence.observed_body_bytes = evidence.observed_body_bytes.saturating_add(data_length);
        }
    }

    /// Stop tracking a finished request and return what was learned.
    pub(super) fn finish(&mut self, request_id: &str, encoded_data_length: Option<u64>) -> Option<HttpBodyEvidence> {
        let mut evidence = self.in_flight.remove(request_id)?;
        evidence.encoded_data_length = encoded_data_length;
        Some(evidence)
    }

    pub(super) fn forget(&mut self, request_id: &str) {
        self.in_flight.remove(request_id);
    }

    pub(super) fn clear(&mut self) {
        self.in_flight.clear();
    }
}

fn content_length_header(headers: &serde_json::Value) -> Option<u64> {
    headers
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.as_str())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn bounded(value: &str, max_bytes: usize) -> String {
    let mut boundary = value.len().min(max_bytes);
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(response: &serde_json::Value, data: &[u64], finished: Option<u64>) -> HttpBodyEvidence {
        let mut table = HttpBodyEvidenceTable::default();
        table.begin("request-1", Some("GET"));
        table.note_response("request-1", response);
        for chunk in data {
            table.note_data("request-1", *chunk);
        }
        table
            .finish("request-1", finished)
            .unwrap_or_else(|| panic!("tracked request must finish"))
    }

    fn empty_result() -> serde_json::Value {
        serde_json::json!({ "body": "", "base64Encoded": false })
    }

    fn unavailable_reason(outcome: CdpBodyOutcome<'_>) -> String {
        match outcome {
            CdpBodyOutcome::Unavailable(reason) => reason,
            CdpBodyOutcome::Captured { .. } => panic!("expected an unavailable body"),
        }
    }

    #[test]
    fn a_blob_drained_pdf_with_a_content_length_is_reported_as_unavailable() {
        let evidence = tracked(
            &serde_json::json!({
                "status": 200,
                "mimeType": "application/pdf",
                "headers": { "content-length": "512357", "Content-Type": "application/pdf" },
                "encodedDataLength": 575
            }),
            &[],
            Some(512_932),
        );

        let reason = unavailable_reason(decode_cdp_body(Some(&evidence), &empty_result()));
        assert!(reason.contains("Content-Length 512357"), "{reason}");
        assert!(reason.contains("application/pdf"), "{reason}");
        assert!(reason.contains("Blob"), "{reason}");
    }

    #[test]
    fn a_blob_drained_chunked_response_is_reported_from_transferred_bytes() {
        let evidence = tracked(
            &serde_json::json!({
                "status": 200,
                "mimeType": "application/pdf",
                "headers": { "Transfer-Encoding": "chunked" },
                "encodedDataLength": 575
            }),
            &[],
            Some(25_000),
        );

        let reason = unavailable_reason(decode_cdp_body(Some(&evidence), &empty_result()));
        assert!(reason.contains("24425 encoded bytes beyond its headers"), "{reason}");
        assert!(reason.contains("Blob"), "{reason}");
    }

    #[test]
    fn an_evicted_body_reports_the_bytes_devtools_observed() {
        let evidence = tracked(
            &serde_json::json!({ "status": 200, "mimeType": "text/plain", "headers": {}, "encodedDataLength": 100 }),
            &[4_096, 4_096],
            Some(8_400),
        );

        let reason = unavailable_reason(decode_cdp_body(Some(&evidence), &empty_result()));
        assert!(reason.contains("observed 8192 decoded body bytes"), "{reason}");
        assert!(reason.contains("did not retain"), "{reason}");
    }

    #[test]
    fn genuinely_empty_responses_stay_captured_empty_bodies() {
        let captured = CdpBodyOutcome::Captured {
            payload: "",
            encoding: BrowserNetworkPayloadEncoding::Text,
            payload_bytes: 0,
        };
        let no_content = tracked(
            &serde_json::json!({ "status": 204, "mimeType": "application/pdf", "headers": {}, "encodedDataLength": 300 }),
            &[],
            Some(9_000),
        );
        assert_eq!(decode_cdp_body(Some(&no_content), &empty_result()), captured);

        let not_modified = tracked(
            &serde_json::json!({ "status": 304, "headers": {}, "encodedDataLength": 300 }),
            &[],
            Some(9_000),
        );
        assert_eq!(decode_cdp_body(Some(&not_modified), &empty_result()), captured);

        let declared_empty = tracked(
            &serde_json::json!({ "status": 200, "mimeType": "application/json", "headers": { "Content-Length": "0" }, "encodedDataLength": 300 }),
            &[],
            Some(300),
        );
        assert_eq!(decode_cdp_body(Some(&declared_empty), &empty_result()), captured);

        let chunked_empty = tracked(
            &serde_json::json!({ "status": 200, "mimeType": "text/html", "headers": { "Transfer-Encoding": "chunked" }, "encodedDataLength": 300 }),
            &[],
            Some(305),
        );
        assert_eq!(decode_cdp_body(Some(&chunked_empty), &empty_result()), captured);

        let mut table = HttpBodyEvidenceTable::default();
        table.begin("head", Some("head"));
        table.note_response(
            "head",
            &serde_json::json!({ "status": 200, "mimeType": "application/pdf", "headers": { "Content-Length": "512357" }, "encodedDataLength": 500 }),
        );
        let head = table
            .finish("head", Some(500))
            .unwrap_or_else(|| panic!("HEAD request must finish"));
        assert_eq!(decode_cdp_body(Some(&head), &empty_result()), captured);
    }

    #[test]
    fn missing_evidence_is_reported_instead_of_assumed_empty() {
        let reason = unavailable_reason(decode_cdp_body(None, &empty_result()));
        assert!(reason.contains("retained no response metadata"), "{reason}");
    }

    #[test]
    fn non_empty_bodies_decode_regardless_of_evidence() {
        let text = serde_json::json!({ "body": "hello", "base64Encoded": false });
        assert_eq!(
            decode_cdp_body(None, &text),
            CdpBodyOutcome::Captured {
                payload: "hello",
                encoding: BrowserNetworkPayloadEncoding::Text,
                payload_bytes: 5,
            }
        );
        let binary = serde_json::json!({ "body": "JVBERi0=", "base64Encoded": true });
        assert_eq!(
            decode_cdp_body(None, &binary),
            CdpBodyOutcome::Captured {
                payload: "JVBERi0=",
                encoding: BrowserNetworkPayloadEncoding::Base64,
                payload_bytes: 5,
            }
        );
        assert_eq!(
            decode_cdp_body(None, &serde_json::json!({ "base64Encoded": true })),
            CdpBodyOutcome::Unavailable("Chromium omitted the response body".to_string())
        );
    }

    #[test]
    fn the_table_is_bounded_and_forgets_failed_or_reset_requests() {
        let mut table = HttpBodyEvidenceTable::default();
        for index in 0..MAX_TRACKED_RESPONSES {
            table.begin(&format!("request-{index}"), Some("GET"));
        }
        table.begin("overflow", Some("GET"));
        assert!(table.finish("overflow", Some(1)).is_none());
        assert!(table.finish("request-7", Some(1)).is_some());

        table.begin("redirected", Some("POST"));
        table.note_data("redirected", 10);
        table.begin("redirected", Some("GET"));
        let redirected = table
            .finish("redirected", None)
            .unwrap_or_else(|| panic!("redirected request must finish"));
        assert_eq!(redirected.method.as_deref(), Some("GET"));
        assert_eq!(redirected.observed_body_bytes, 0);

        table.begin("failed", Some("GET"));
        table.forget("failed");
        assert!(table.finish("failed", None).is_none());

        table.begin("cleared", Some("GET"));
        table.clear();
        assert!(table.finish("cleared", None).is_none());
    }

    #[test]
    fn response_metadata_is_parsed_case_insensitively_and_bounded() {
        let mut table = HttpBodyEvidenceTable::default();
        table.begin("request-1", Some(&"M".repeat(MAX_METHOD_BYTES + 8)));
        table.note_response(
            "request-1",
            &serde_json::json!({
                "status": 200,
                "mimeType": "x".repeat(MAX_MIME_TYPE_BYTES + 8),
                "headers": { "CONTENT-LENGTH": " 42 " },
                "encodedDataLength": 17
            }),
        );
        let evidence = table
            .finish("request-1", Some(60))
            .unwrap_or_else(|| panic!("request must finish"));

        assert_eq!(evidence.method.as_ref().map(String::len), Some(MAX_METHOD_BYTES));
        assert_eq!(evidence.mime_type.as_ref().map(String::len), Some(MAX_MIME_TYPE_BYTES));
        assert_eq!(evidence.content_length, Some(42));
        assert_eq!(evidence.header_bytes, Some(17));
        assert_eq!(evidence.encoded_data_length, Some(60));
        assert_eq!(evidence.unaccounted_encoded_bytes(), 43);
    }
}
