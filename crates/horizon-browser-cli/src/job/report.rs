use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use horizon_browser::BackendKind;
use horizon_browser_protocol::redact_url;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{Plan, PlanStep, observability::ObservabilitySummary};

use super::{JobError, JobOptions, create_private, io_error, write_private};

const TRACE_NAME: &str = "trace.jsonl";
const PLAN_NAME: &str = "executed-plan.json";
const REPORT_NAME: &str = "report.json";
const MAX_TRACE_CALLS: usize = 256;
const MAX_TRACE_BYTES: usize = 1024 * 1024;

pub(super) struct JobTrace {
    writer: File,
    trace_path: PathBuf,
    calls: Vec<RecordedCall>,
    replayable: bool,
    trace_bytes: usize,
}

struct RecordedCall {
    tool: String,
    arguments: Map<String, Value>,
    ok: bool,
    health: Option<Value>,
}

#[derive(Serialize)]
struct TraceRecord<'a> {
    sequence: usize,
    tool: &'a str,
    arguments: &'a Map<String, Value>,
    ok: bool,
}

#[derive(Serialize)]
struct JobReport<'a> {
    version: u32,
    ok: bool,
    backend: &'static str,
    visibility: &'static str,
    summary: &'a str,
    artifact: Option<String>,
    browser_cleanup_ok: bool,
    tool_calls: usize,
    replayable: bool,
    trace: String,
    executed_plan: String,
    observability: ObservabilitySummary,
}

pub(super) struct ReportArtifacts {
    pub(super) report: PathBuf,
    pub(super) plan: PathBuf,
    pub(super) trace: PathBuf,
    pub(super) replayable: bool,
}

pub(super) struct ReportInput<'a> {
    pub(super) options: &'a JobOptions,
    pub(super) backend: BackendKind,
    pub(super) ok: bool,
    pub(super) summary: &'a str,
    pub(super) artifact: Option<&'a Path>,
    pub(super) browser_cleanup_ok: bool,
}

impl JobTrace {
    pub(super) fn start(job_dir: &Path) -> Result<Self, JobError> {
        let trace_path = job_dir.join(TRACE_NAME);
        Ok(Self {
            writer: create_private(&trace_path)?,
            trace_path,
            calls: Vec::new(),
            replayable: true,
            trace_bytes: 0,
        })
    }

    pub(super) fn record_line(&mut self, line: &str) -> Result<Option<String>, JobError> {
        let Some(mut call) = parse_tool_call(line) else {
            return Ok(None);
        };
        if self.calls.len() >= MAX_TRACE_CALLS {
            return Err(JobError::Result(
                "agent exceeded the 256-call executed-plan limit".to_string(),
            ));
        }
        if !call.ok || !redact_arguments(&mut call.arguments) {
            self.replayable = false;
        }
        let record = TraceRecord {
            sequence: self.calls.len() + 1,
            tool: &call.tool,
            arguments: &call.arguments,
            ok: call.ok,
        };
        let mut record_bytes = serde_json::to_vec(&record)
            .map_err(|error| JobError::Result(format!("could not encode MCP trace: {error}")))?;
        record_bytes.push(b'\n');
        let Some(trace_bytes) = self.trace_bytes.checked_add(record_bytes.len()) else {
            return Err(trace_limit_error());
        };
        if trace_bytes > MAX_TRACE_BYTES {
            return Err(trace_limit_error());
        }
        self.writer
            .write_all(&record_bytes)
            .map_err(|source| io_error("could not write MCP trace", &source))?;
        self.trace_bytes = trace_bytes;
        let tool = call.tool.clone();
        self.calls.push(call);
        Ok(Some(tool))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub(super) fn finish(mut self, job_dir: &Path, input: &ReportInput<'_>) -> Result<ReportArtifacts, JobError> {
        self.writer
            .flush()
            .and_then(|()| self.writer.sync_all())
            .map_err(|source| io_error("could not finish MCP trace", &source))?;
        let plan_path = job_dir.join(PLAN_NAME);
        let report_path = job_dir.join(REPORT_NAME);
        let (plan, replayable) = self.executed_plan();
        let mut plan_bytes = serde_json::to_vec_pretty(&plan)
            .map_err(|error| JobError::Result(format!("could not encode executed plan: {error}")))?;
        Plan::from_slice(&plan_bytes)
            .map_err(|error| JobError::Result(format!("executed MCP trace did not form a valid plan: {error}")))?;
        plan_bytes.push(b'\n');
        write_private(&plan_path, &plan_bytes)?;
        let report = JobReport {
            version: 1,
            ok: input.ok && input.browser_cleanup_ok,
            backend: backend_name(input.backend),
            visibility: if input.options.visible { "visible" } else { "hidden" },
            summary: input.summary,
            artifact: input.artifact.map(|path| path.display().to_string()),
            browser_cleanup_ok: input.browser_cleanup_ok,
            tool_calls: self.calls.len(),
            replayable,
            trace: self.trace_path.display().to_string(),
            executed_plan: plan_path.display().to_string(),
            observability: ObservabilitySummary::from_results(
                self.calls
                    .iter()
                    .filter_map(|call| call.health.as_ref().map(|health| (call.tool.as_str(), health))),
            ),
        };
        let mut report_bytes = serde_json::to_vec_pretty(&report)
            .map_err(|error| JobError::Result(format!("could not encode job report: {error}")))?;
        report_bytes.push(b'\n');
        write_private(&report_path, &report_bytes)?;
        Ok(ReportArtifacts {
            report: report_path,
            plan: plan_path,
            trace: self.trace_path,
            replayable,
        })
    }

    fn executed_plan(&mut self) -> (Plan, bool) {
        let list_index = self
            .calls
            .iter()
            .position(|call| call.tool == "browser_list" && call.ok);
        if list_index != Some(0) {
            self.replayable = false;
        }
        let list_step = list_index.map(step_id);
        let steps = self
            .calls
            .iter()
            .enumerate()
            .map(|(index, call)| {
                let mut arguments = call.arguments.clone();
                if index > list_index.unwrap_or(usize::MAX)
                    && arguments.contains_key("panel_id")
                    && let Some(step) = &list_step
                {
                    arguments.insert(
                        "panel_id".to_string(),
                        json!({"$ref": format!("{step}#/panels/0/panel_id")}),
                    );
                }
                if contains_ephemeral_reference(&arguments) {
                    self.replayable = false;
                }
                PlanStep {
                    id: step_id(index),
                    tool: call.tool.clone(),
                    arguments,
                }
            })
            .collect();
        (Plan { version: 1, steps }, self.replayable)
    }
}

fn parse_tool_call(line: &str) -> Option<RecordedCall> {
    let event: Value = serde_json::from_str(line).ok()?;
    if event.get("type")?.as_str()? != "item.completed" {
        return None;
    }
    let item = event.get("item")?;
    if item.get("type")?.as_str()? != "mcp_tool_call" || item.get("server")?.as_str()? != "horizon-browser" {
        return None;
    }
    let tool = item.get("tool")?.as_str()?.to_string();
    let result = item
        .get("result")
        .or_else(|| item.get("output"))
        .or_else(|| item.get("structured_content"));
    Some(RecordedCall {
        arguments: item.get("arguments")?.as_object()?.clone(),
        ok: item.get("status").and_then(Value::as_str) == Some("completed")
            && item.get("error").is_none_or(Value::is_null),
        health: result.and_then(|result| ObservabilitySummary::health_payload(&tool, result)),
        tool,
    })
}

fn redact_arguments(arguments: &mut Map<String, Value>) -> bool {
    let mut replayable = true;
    redact_map(arguments, &mut replayable);
    replayable
}

fn redact_map(values: &mut Map<String, Value>, replayable: &mut bool) {
    for (key, value) in values {
        match key.as_str() {
            "url" => {
                if let Some(url) = value.as_str() {
                    let redacted = redact_url(url);
                    *replayable &= redacted == url;
                    *value = Value::String(redacted);
                }
            }
            "url_patterns" => redact_url_patterns(value, replayable),
            "body" | "data" | "expression" | "headers" | "password" | "reason" | "script" | "selector" | "text"
            | "token" | "value" => {
                if !value.is_null() {
                    *value = Value::String("<redacted>".to_string());
                    *replayable = false;
                }
            }
            _ => redact_value(value, replayable),
        }
    }
}

fn redact_url_patterns(value: &mut Value, replayable: &mut bool) {
    if let Value::Array(patterns) = value {
        for pattern in patterns {
            if !pattern.is_null() {
                *pattern = Value::String("<redacted>".to_string());
                *replayable = false;
            }
        }
    } else if !value.is_null() {
        *value = Value::String("<redacted>".to_string());
        *replayable = false;
    }
}

fn redact_value(value: &mut Value, replayable: &mut bool) {
    match value {
        Value::Object(values) => redact_map(values, replayable),
        Value::Array(values) => {
            for value in values {
                redact_value(value, replayable);
            }
        }
        _ => {}
    }
}

fn contains_ephemeral_reference(arguments: &Map<String, Value>) -> bool {
    arguments.iter().any(|(key, value)| {
        matches!(
            key.as_str(),
            "action_id" | "capture_id" | "cursor" | "ref" | "request_id"
        ) && !value.is_null()
            || match value {
                Value::Object(values) => contains_ephemeral_reference(values),
                Value::Array(values) => values.iter().any(|value| match value {
                    Value::Object(values) => contains_ephemeral_reference(values),
                    _ => false,
                }),
                _ => false,
            }
    })
}

fn step_id(index: usize) -> String {
    format!("step-{:03}", index + 1)
}

fn trace_limit_error() -> JobError {
    JobError::Result("agent exceeded the 1 MiB redacted MCP trace limit".to_string())
}

const fn backend_name(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::ChromiumCdp => "chromium",
        BackendKind::FirefoxBidi => "firefox",
        BackendKind::SafariWebDriver => "safari",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_calls_become_a_valid_replayable_plan() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        trace
            .record_line(&event("browser_list", &json!({})))
            .expect("list event");
        trace
            .record_line(&event(
                "browser_navigate",
                &json!({"panel_id":"standalone-1", "url":"https://example.com"}),
            ))
            .expect("navigate event");
        let options = JobOptions {
            prompt: "visit example.com".to_string(),
            backend: None,
            visible: false,
            json: true,
        };
        let artifacts = trace
            .finish(
                directory.path(),
                &ReportInput {
                    options: &options,
                    backend: BackendKind::FirefoxBidi,
                    ok: true,
                    summary: "done",
                    artifact: None,
                    browser_cleanup_ok: true,
                },
            )
            .expect("report artifacts");

        assert!(artifacts.replayable);
        let plan = Plan::from_slice(&std::fs::read(&artifacts.plan).expect("plan bytes")).expect("validated plan");
        assert_eq!(
            plan.steps[1].arguments["panel_id"],
            json!({"$ref":"step-001#/panels/0/panel_id"})
        );
        let report: Value =
            serde_json::from_slice(&std::fs::read(&artifacts.report).expect("report bytes")).expect("validated report");
        assert_eq!(report["backend"], "firefox");
        assert_eq!(report["observability"]["audit"]["observed"], false);
        assert_eq!(report["observability"]["network"]["observed"], false);
    }

    #[test]
    fn audit_and_network_results_are_summarized_without_payloads() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        trace
            .record_line(&event_with_result(
                "browser_audit",
                &json!({"panel_id":"p1"}),
                &json!({
                    "records_retained": 3,
                    "records_returned": 3,
                    "malformed_records": 1,
                    "older_records_dropped": 2,
                    "cursor_lost": true,
                    "has_more": false,
                    "entries": [{"event_id":"secret"}]
                }),
            ))
            .expect("audit event");
        trace
            .record_line(&event_with_result(
                "browser_network_watch",
                &json!({"panel_id":"p1"}),
                &json!({
                    "sequence_gaps": 4,
                    "records_dropped": 1,
                    "writer_failed": true,
                    "records": [{"payload":"secret"}]
                }),
            ))
            .expect("watch event");
        let options = JobOptions {
            prompt: "observe".to_string(),
            backend: None,
            visible: false,
            json: true,
        };
        let artifacts = trace
            .finish(
                directory.path(),
                &ReportInput {
                    options: &options,
                    backend: BackendKind::ChromiumCdp,
                    ok: true,
                    summary: "done",
                    artifact: None,
                    browser_cleanup_ok: true,
                },
            )
            .expect("report artifacts");
        let report: Value =
            serde_json::from_slice(&std::fs::read(&artifacts.report).expect("report bytes")).expect("validated report");
        assert_eq!(report["observability"]["audit"]["observed"], true);
        assert_eq!(report["observability"]["audit"]["records_retained"], 3);
        assert_eq!(report["observability"]["audit"]["older_records_dropped"], 2);
        assert_eq!(report["observability"]["audit"]["cursor_lost"], true);
        assert_eq!(report["observability"]["network"]["sequence_gaps"], 4);
        assert_eq!(report["observability"]["network"]["writer_failed"], true);
        let report_text = report.to_string();
        assert!(!report_text.contains("secret"));
        assert!(!report_text.contains("payload"));
    }

    #[test]
    fn sensitive_arguments_are_redacted_and_not_replayable() {
        let mut arguments = json!({
            "url":"https://example.com/path?token=secret#fragment",
            "url_patterns":["token=secret", "https://example.com/public", null],
            "value":"private text"
        })
        .as_object()
        .cloned()
        .expect("object");

        assert!(!redact_arguments(&mut arguments));
        assert_eq!(arguments["url"], "https://example.com/path?<redacted>#<redacted>");
        assert_eq!(arguments["url_patterns"], json!(["<redacted>", "<redacted>", null]));
        assert_eq!(arguments["value"], "<redacted>");
    }

    #[test]
    fn aggregate_trace_size_is_bounded_before_retaining_another_call() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        let payload = "x".repeat(MAX_TRACE_BYTES / 2);
        trace
            .record_line(&event("browser_query", &json!({"custom": &payload})))
            .expect("first bounded event");

        let error = trace
            .record_line(&event("browser_query", &json!({"custom": &payload})))
            .expect_err("second event must exceed aggregate trace limit");

        assert!(matches!(error, JobError::Result(message) if message.contains("1 MiB")));
        assert_eq!(trace.calls.len(), 1);
        assert!(trace.trace_bytes <= MAX_TRACE_BYTES);
    }

    fn event(tool: &str, arguments: &Value) -> String {
        event_with_result(tool, arguments, &Value::Null)
    }

    fn event_with_result(tool: &str, arguments: &Value, result: &Value) -> String {
        json!({
            "type":"item.completed",
            "item":{
                "type":"mcp_tool_call",
                "server":"horizon-browser",
                "tool":tool,
                "arguments":arguments,
                "status":"completed",
                "error":null,
                "result":{
                    "content":[],
                    "structured_content":result
                }
            }
        })
        .to_string()
    }
}
