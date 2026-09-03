//! Capture health and audit-completeness summaries for job reports.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

/// Loss and completeness counters collected from executed MCP results.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ObservabilitySummary {
    /// Last `browser_audit` page metadata, if the job observed the journal.
    pub audit: AuditObservability,
    /// Aggregated `browser_network` / `browser_network_watch` health.
    pub network: NetworkObservability,
}

/// Completeness of the retained action journal as last observed.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AuditObservability {
    /// True when at least one `browser_audit` result was recorded.
    pub observed: bool,
    /// Number of successful `browser_audit` results.
    #[serde(skip_serializing_if = "is_zero")]
    pub pages: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records_retained: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records_returned: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub malformed_records: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_records_dropped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_lost: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Capture-loss counters aggregated across network tools.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NetworkObservability {
    /// True when a network start, status, stop, or watch result was recorded.
    pub observed: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub sequence_gaps: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub records_written: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub records_dropped: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub records_enqueued: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub payloads_truncated: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub connections_truncated: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub connection_urls_truncated: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub returned_payloads_truncated: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub malformed_records: u64,
    #[serde(flatten)]
    pub loss: NetworkLoss,
}

/// Capture writer and file-limit flags.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NetworkLoss {
    #[serde(skip_serializing_if = "is_false")]
    pub file_limit_reached: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub writer_failed: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub file_reset: bool,
}

impl ObservabilitySummary {
    /// Build a summary from MCP structured results, ignoring unrelated tools.
    #[must_use]
    pub fn from_results<'a, I>(results: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a Value)>,
    {
        let mut summary = Self::default();
        let mut captures = BTreeMap::<String, CaptureTotals>::new();
        let mut anonymous = CaptureTotals::default();
        let mut saw_network = false;
        for (tool, result) in results {
            let result = structured_content(result);
            match tool {
                "browser_audit" => summary.audit.observe(result),
                "browser_network" | "browser_network_watch" => {
                    saw_network = true;
                    match result
                        .get("capture_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                    {
                        Some(capture_id) => captures.entry(capture_id.to_string()).or_default().observe(result),
                        None => anonymous.observe(result),
                    }
                }
                _ => {}
            }
        }
        if saw_network {
            summary.network.observed = true;
            for capture in captures.values().chain(std::iter::once(&anonymous)) {
                summary.network.absorb(capture);
            }
        }
        summary
    }

    /// Keep only bounded health fields from a tool result. Payload arrays are dropped.
    #[must_use]
    pub fn health_payload(tool: &str, result: &Value) -> Option<Value> {
        if !matches!(tool, "browser_audit" | "browser_network" | "browser_network_watch") {
            return None;
        }
        let mut object = structured_content(result).as_object()?.clone();
        object.remove("entries");
        object.remove("records");
        object.remove("known_connections");
        Some(Value::Object(object))
    }
}

impl AuditObservability {
    fn observe(&mut self, result: &Value) {
        self.observed = true;
        self.pages = self.pages.saturating_add(1);
        self.records_retained = u64_field(result, "records_retained").or(self.records_retained);
        self.records_returned = u64_field(result, "records_returned").or(self.records_returned);
        self.malformed_records = u64_field(result, "malformed_records").or(self.malformed_records);
        self.older_records_dropped = u64_field(result, "older_records_dropped").or(self.older_records_dropped);
        self.cursor_lost = bool_field(result, "cursor_lost").or(self.cursor_lost);
        self.has_more = bool_field(result, "has_more").or(self.has_more);
    }
}

impl NetworkObservability {
    fn absorb(&mut self, capture: &CaptureTotals) {
        self.sequence_gaps = self.sequence_gaps.saturating_add(capture.sequence_gaps);
        self.records_written = self.records_written.saturating_add(capture.records_written);
        self.records_dropped = self.records_dropped.saturating_add(capture.records_dropped);
        self.records_enqueued = self.records_enqueued.saturating_add(capture.records_enqueued);
        self.payloads_truncated = self.payloads_truncated.saturating_add(capture.payloads_truncated);
        self.connections_truncated = self.connections_truncated.saturating_add(capture.connections_truncated);
        self.connection_urls_truncated = self
            .connection_urls_truncated
            .saturating_add(capture.connection_urls_truncated);
        self.returned_payloads_truncated = self
            .returned_payloads_truncated
            .saturating_add(capture.returned_payloads_truncated);
        self.malformed_records = self.malformed_records.saturating_add(capture.malformed_records);
        self.loss.file_limit_reached |= capture.file_limit_reached;
        self.loss.writer_failed |= capture.writer_failed;
        self.loss.file_reset |= capture.file_reset;
    }
}

#[derive(Clone, Debug, Default)]
struct CaptureTotals {
    records_written: u64,
    records_dropped: u64,
    records_enqueued: u64,
    payloads_truncated: u64,
    connections_truncated: u64,
    sequence_gaps: u64,
    returned_payloads_truncated: u64,
    malformed_records: u64,
    connection_urls_truncated: u64,
    file_limit_reached: bool,
    writer_failed: bool,
    file_reset: bool,
}

impl CaptureTotals {
    fn observe(&mut self, result: &Value) {
        self.records_written = self
            .records_written
            .max(u64_field(result, "records_written").unwrap_or(0));
        self.records_dropped = self
            .records_dropped
            .max(u64_field(result, "records_dropped").unwrap_or(0));
        self.records_enqueued = self
            .records_enqueued
            .max(u64_field(result, "records_enqueued").unwrap_or(0));
        self.payloads_truncated = self
            .payloads_truncated
            .max(u64_field(result, "payloads_truncated").unwrap_or(0));
        self.connections_truncated = self
            .connections_truncated
            .max(u64_field(result, "connections_truncated").unwrap_or(0));
        self.sequence_gaps = self
            .sequence_gaps
            .saturating_add(u64_field(result, "sequence_gaps").unwrap_or(0));
        self.returned_payloads_truncated = self
            .returned_payloads_truncated
            .saturating_add(u64_field(result, "returned_payloads_truncated").unwrap_or(0));
        self.malformed_records = self
            .malformed_records
            .saturating_add(u64_field(result, "malformed_records").unwrap_or(0));
        self.connection_urls_truncated = self
            .connection_urls_truncated
            .saturating_add(u64_field(result, "connection_urls_truncated").unwrap_or(0));
        self.file_limit_reached |= bool_field(result, "file_limit_reached").unwrap_or(false);
        self.writer_failed |= bool_field(result, "writer_failed").unwrap_or(false);
        self.file_reset |= bool_field(result, "file_reset").unwrap_or(false);
    }
}

fn structured_content(result: &Value) -> &Value {
    result.get("structured_content").unwrap_or(result)
}

fn u64_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn unrelated_tools_are_not_observed() {
        let summary = ObservabilitySummary::from_results([("browser_list", &json!({"panels":[]}))]);
        assert!(!summary.audit.observed);
        assert!(!summary.network.observed);
    }

    #[test]
    fn last_audit_page_is_the_completeness_snapshot() {
        let first = json!({
            "records_retained": 4,
            "records_returned": 2,
            "malformed_records": 0,
            "older_records_dropped": 1,
            "cursor_lost": false,
            "has_more": true,
            "entries": [{"event_id":"e1"}]
        });
        let second = json!({
            "records_retained": 4,
            "records_returned": 2,
            "malformed_records": 1,
            "older_records_dropped": 1,
            "cursor_lost": false,
            "has_more": false
        });
        let summary = ObservabilitySummary::from_results([("browser_audit", &first), ("browser_audit", &second)]);
        assert!(summary.audit.observed);
        assert_eq!(summary.audit.pages, 2);
        assert_eq!(summary.audit.records_retained, Some(4));
        assert_eq!(summary.audit.records_returned, Some(2));
        assert_eq!(summary.audit.malformed_records, Some(1));
        assert_eq!(summary.audit.older_records_dropped, Some(1));
        assert_eq!(summary.audit.has_more, Some(false));
        assert!(
            ObservabilitySummary::health_payload("browser_audit", &first)
                .is_some_and(|payload| payload.get("entries").is_none())
        );
    }

    #[test]
    fn network_watch_and_status_counters_are_merged() {
        let status = json!({
            "capture_id": "cap-1",
            "records_written": 10,
            "records_dropped": 2,
            "records_enqueued": 12,
            "payloads_truncated": 1,
            "connections_truncated": 0,
            "file_limit_reached": false,
            "writer_failed": false,
            "records": [{"sequence":1}]
        });
        let watch = json!({
            "capture_id": "cap-1",
            "sequence_gaps": 3,
            "records_dropped": 2,
            "payloads_truncated": 1,
            "returned_payloads_truncated": 4,
            "connection_urls_truncated": 5,
            "malformed_records": 1,
            "file_limit_reached": true,
            "writer_failed": true,
            "file_reset": true
        });
        let summary =
            ObservabilitySummary::from_results([("browser_network", &status), ("browser_network_watch", &watch)]);
        assert!(summary.network.observed);
        assert_eq!(summary.network.sequence_gaps, 3);
        assert_eq!(summary.network.records_written, 10);
        assert_eq!(summary.network.records_dropped, 2);
        assert_eq!(summary.network.returned_payloads_truncated, 4);
        assert_eq!(summary.network.connection_urls_truncated, 5);
        assert_eq!(summary.network.malformed_records, 1);
        assert!(summary.network.loss.file_limit_reached);
        assert!(summary.network.loss.writer_failed);
        assert!(summary.network.loss.file_reset);
        assert!(
            ObservabilitySummary::health_payload("browser_network", &status)
                .is_some_and(|payload| payload.get("records").is_none())
        );
    }

    #[test]
    fn per_capture_totals_are_summed_and_envelopes_are_unwrapped() {
        let first = json!({
            "capture_id": "a",
            "records_written": 10,
            "records_dropped": 1
        });
        let first_again = json!({
            "capture_id": "a",
            "records_written": 12,
            "records_dropped": 1
        });
        let second = json!({
            "structured_content": {
                "capture_id": "b",
                "records_written": 7,
                "records_dropped": 2,
                "connection_urls_truncated": 3
            }
        });
        let summary = ObservabilitySummary::from_results([
            ("browser_network", &first),
            ("browser_network", &first_again),
            ("browser_network_watch", &second),
        ]);
        assert_eq!(summary.network.records_written, 19);
        assert_eq!(summary.network.records_dropped, 3);
        assert_eq!(summary.network.connection_urls_truncated, 3);
        let envelope = json!({
            "content": [],
            "structured_content": {
                "records_retained": 8,
                "entries": [{"event_id":"hidden"}]
            }
        });
        let payload = ObservabilitySummary::health_payload("browser_audit", &envelope).expect("payload");
        assert_eq!(payload["records_retained"], 8);
        assert!(payload.get("entries").is_none());
        assert!(payload.get("structured_content").is_none());
    }
}
