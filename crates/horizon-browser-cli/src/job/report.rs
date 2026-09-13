use std::collections::HashMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use horizon_browser::BackendKind;
use horizon_browser_protocol::redact_url;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{Plan, PlanStep, observability::ObservabilitySummary};

use super::{JobError, JobOptions, create_private, io_error, write_private};

const MCP_SERVER: &str = "horizon-browser";
const MCP_PREFIX: &str = "horizon-browser__";

const TRACE_NAME: &str = "trace.jsonl";
const PLAN_NAME: &str = "executed-plan.json";
const REPORT_NAME: &str = "report.json";
const MAX_TRACE_CALLS: usize = 256;
const MAX_TRACE_BYTES: usize = 1024 * 1024;

pub(super) struct JobTrace {
    writer: File,
    trace_path: PathBuf,
    calls: Vec<RecordedCall>,
    pending: HashMap<String, PendingCall>,
    grok_text: String,
    structured_result: Option<Value>,
    replayable: bool,
    trace_bytes: usize,
}

struct PendingCall {
    tool: String,
    arguments: Map<String, Value>,
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
            pending: HashMap::new(),
            grok_text: String::new(),
            structured_result: None,
            replayable: true,
            trace_bytes: 0,
        })
    }

    pub(super) fn record_line(&mut self, line: &str) -> Result<Option<String>, JobError> {
        if let Some(result) = parse_structured_result(line) {
            self.structured_result = Some(result);
        }
        self.observe_grok_text(line);
        let Some(call) = parse_tool_call(line).or_else(|| self.parse_grok_tool_event(line)) else {
            return Ok(None);
        };
        self.record_call(call)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    pub(super) fn structured_result_bytes(&self) -> Option<Vec<u8>> {
        let value = self
            .structured_result
            .clone()
            .or_else(|| last_agent_result(&self.grok_text))?;
        serde_json::to_vec(&json!({
            "ok": value.get("ok")?,
            "summary": value.get("summary")?,
            "artifact_content": value.get("artifact_content")?,
        }))
        .ok()
    }

    fn record_call(&mut self, mut call: RecordedCall) -> Result<Option<String>, JobError> {
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

    fn observe_grok_text(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(data) = event.get("data").and_then(Value::as_str) {
                    self.grok_text.push_str(data);
                }
            }
            Some("end") => {
                if let Some(result) = event
                    .get("structured_output")
                    .cloned()
                    .and_then(value_as_agent_result)
                    .or_else(|| last_agent_result(&self.grok_text))
                {
                    self.structured_result = Some(result);
                }
            }
            _ => {}
        }
    }

    fn parse_grok_tool_event(&mut self, line: &str) -> Option<RecordedCall> {
        let event: Value = serde_json::from_str(line).ok()?;
        match event.get("type")?.as_str()? {
            "tool_call" => {
                let id = event.get("toolCallId")?.as_str()?.to_string();
                let tool_name = event.get("toolName")?.as_str()?;
                let input = event.get("rawInput").cloned().unwrap_or(Value::Null);
                let (tool, arguments) = horizon_browser_call(tool_name, &input)?;
                let pending = PendingCall { tool, arguments };
                if event.get("status").and_then(Value::as_str) == Some("completed") {
                    return Some(completed_grok_call(pending, event.get("rawOutput"), true));
                }
                self.pending.insert(id, pending);
                None
            }
            "tool_call_update" => {
                let id = event.get("toolCallId")?.as_str()?;
                let status = event.get("status").and_then(Value::as_str).unwrap_or_default();
                if !matches!(status, "completed" | "failed") {
                    return None;
                }
                let pending = self.pending.remove(id)?;
                Some(completed_grok_call(
                    pending,
                    event.get("rawOutput"),
                    status == "completed",
                ))
            }
            _ => None,
        }
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
    if item.get("type")?.as_str()? != "mcp_tool_call" || item.get("server")?.as_str()? != MCP_SERVER {
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

fn parse_structured_result(line: &str) -> Option<Value> {
    let event: Value = serde_json::from_str(line).ok()?;
    if let Some(result) = event.get("structured_output").cloned().and_then(value_as_agent_result) {
        return Some(result);
    }
    if event.get("type").and_then(Value::as_str) == Some("end") {
        return event
            .get("text")
            .and_then(Value::as_str)
            .and_then(parse_result_json)
            .or_else(|| event.get("structured_output").cloned().and_then(value_as_agent_result));
    }
    if event.get("ok").is_some() && event.get("summary").is_some() && event.get("artifact_content").is_some() {
        return value_as_agent_result(event);
    }
    if event.get("type").is_none()
        && let Some(text) = event.get("text").and_then(Value::as_str)
    {
        return parse_result_json(text);
    }
    None
}

fn parse_result_json(text: &str) -> Option<Value> {
    last_agent_result(text)
}

fn last_agent_result(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .map_or(trimmed, |value| value.strip_suffix("```").unwrap_or(value).trim());
    let mut last = None;
    let mut offset = 0;
    while let Some(relative) = unfenced[offset..].find('{') {
        offset += relative;
        match json_object_at(unfenced, offset) {
            Some((value, end)) => {
                if let Some(result) = value_as_agent_result(value) {
                    last = Some(result);
                }
                offset = end;
            }
            None => offset += 1,
        }
    }
    last
}

fn json_object_at(text: &str, start: usize) -> Option<(Value, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(start).copied() != Some(b'{') {
        return None;
    }
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let end = index + 1;
                    let value = serde_json::from_str(&text[start..end]).ok()?;
                    return Some((value, end));
                }
            }
            _ => {}
        }
    }
    None
}

fn value_as_agent_result(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    object.get("ok")?.as_bool()?;
    object.get("summary")?.as_str()?;
    object.contains_key("artifact_content").then_some(value)
}

fn horizon_browser_call(tool_name: &str, input: &Value) -> Option<(String, Map<String, Value>)> {
    if let Some(tool) = tool_name.strip_prefix(MCP_PREFIX) {
        return Some((tool.to_string(), object_args(input)));
    }
    if tool_name != "use_tool" {
        return None;
    }
    let qualified = input
        .get("tool_name")
        .or_else(|| input.get("name"))
        .or_else(|| input.get("tool"))
        .and_then(Value::as_str)?;
    let tool = qualified.strip_prefix(MCP_PREFIX)?;
    let arguments = input
        .get("tool_input")
        .or_else(|| input.get("arguments"))
        .or_else(|| input.get("input"))
        .unwrap_or(&Value::Null);
    Some((tool.to_string(), object_args(arguments)))
}

fn object_args(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn completed_grok_call(pending: PendingCall, output: Option<&Value>, ok: bool) -> RecordedCall {
    let result = output.cloned().unwrap_or(Value::Null);
    RecordedCall {
        health: ObservabilitySummary::health_payload(&pending.tool, &result),
        tool: pending.tool,
        arguments: pending.arguments,
        ok,
    }
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

    #[test]
    fn grok_use_tool_events_record_horizon_browser_calls() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        assert!(
            trace
                .record_line(
                    &json!({
                        "type":"tool_call",
                        "toolCallId":"call_1",
                        "toolName":"use_tool",
                        "status":"in_progress",
                        "rawInput":{
                            "tool_name":"horizon-browser__browser_list",
                            "tool_input":{}
                        }
                    })
                    .to_string()
                )
                .expect("start")
                .is_none()
        );
        assert_eq!(
            trace
                .record_line(
                    &json!({
                        "type":"tool_call_update",
                        "toolCallId":"call_1",
                        "status":"completed",
                        "rawOutput":{"panels":[{"panel_id":"p1"}]}
                    })
                    .to_string()
                )
                .expect("complete")
                .as_deref(),
            Some("browser_list")
        );
        assert!(
            trace
                .record_line(
                    &json!({
                        "type":"tool_call",
                        "toolCallId":"call_2",
                        "toolName":"search_tool",
                        "rawInput":{"query":"browser"}
                    })
                    .to_string()
                )
                .expect("search")
                .is_none()
        );
        assert_eq!(
            trace
                .record_line(
                    &json!({
                        "type":"tool_call",
                        "toolCallId":"call_3",
                        "toolName":"horizon-browser__browser_navigate",
                        "status":"completed",
                        "rawInput":{"panel_id":"p1","url":"example.com"},
                        "rawOutput":{"completed":true}
                    })
                    .to_string()
                )
                .expect("navigate")
                .as_deref(),
            Some("browser_navigate")
        );
        assert!(!trace.is_empty());
        assert_eq!(trace.calls.len(), 2);
        assert_eq!(trace.calls[0].tool, "browser_list");
        assert_eq!(trace.calls[1].arguments["url"], "example.com");

        let failed = trace
            .record_line(
                &json!({
                    "type":"tool_call",
                    "toolCallId":"call_4",
                    "toolName":"use_tool",
                    "rawInput":{"name":"horizon-browser__browser_wait","arguments":{"panel_id":"p1"}},
                    "status":"in_progress"
                })
                .to_string(),
            )
            .expect("wait start");
        assert!(failed.is_none());
        assert_eq!(
            trace
                .record_line(
                    &json!({
                        "type":"tool_call_update",
                        "toolCallId":"call_4",
                        "status":"failed",
                        "rawOutput":{"error":"timeout"}
                    })
                    .to_string()
                )
                .expect("wait failed")
                .as_deref(),
            Some("browser_wait")
        );
        assert!(!trace.calls[2].ok);
    }

    #[test]
    fn grok_text_and_end_events_capture_the_structured_result() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        trace
            .record_line(&json!({"type":"text","data":"```json\n"}).to_string())
            .expect("fence open");
        trace
            .record_line(
                &json!({
                    "type":"text",
                    "data":"{\"ok\":true,\"summary\":\"Example Domain\",\"artifact_content\":\"Example Domain\"}\n"
                })
                .to_string(),
            )
            .expect("json text");
        trace
            .record_line(&json!({"type":"text","data":"```"}).to_string())
            .expect("fence close");
        trace
            .record_line(&json!({"type":"end","stopReason":"end_turn"}).to_string())
            .expect("end");
        let bytes = trace.structured_result_bytes().expect("structured result");
        let result: Value = serde_json::from_slice(&bytes).expect("decode result");
        assert_eq!(result["ok"], true);
        assert_eq!(result["summary"], "Example Domain");
        assert_eq!(result["artifact_content"], "Example Domain");
    }

    #[test]
    fn later_grok_result_json_replaces_an_earlier_planning_object() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        trace
            .record_line(
                &json!({
                    "type":"text",
                    "data":"{\"ok\":true,\"summary\":\"planning\",\"artifact_content\":null}\n"
                })
                .to_string(),
            )
            .expect("planning json");
        trace
            .record_line(
                &json!({
                    "type":"text",
                    "data":"{\"ok\":true,\"summary\":\"Example Domain\",\"artifact_content\":\"Example Domain\"}\n"
                })
                .to_string(),
            )
            .expect("final json");
        trace
            .record_line(&json!({"type":"end","stopReason":"end_turn"}).to_string())
            .expect("end");
        let bytes = trace.structured_result_bytes().expect("structured result");
        let result: Value = serde_json::from_slice(&bytes).expect("decode result");
        assert_eq!(result["summary"], "Example Domain");
        assert_eq!(result["artifact_content"], "Example Domain");
    }

    #[test]
    fn grok_json_document_text_is_accepted_as_the_structured_result() {
        let directory = tempfile::tempdir().expect("job directory");
        let mut trace = JobTrace::start(directory.path()).expect("trace");
        trace
            .record_line(
                &json!({
                    "text":"{\"ok\":false,\"summary\":\"blocked\",\"artifact_content\":null}",
                    "stopReason":"end_turn",
                    "sessionId":"abc"
                })
                .to_string(),
            )
            .expect("json document");
        let bytes = trace.structured_result_bytes().expect("structured result");
        let result: Value = serde_json::from_slice(&bytes).expect("decode result");
        assert_eq!(result["ok"], false);
        assert_eq!(result["summary"], "blocked");
        assert_eq!(result["artifact_content"], Value::Null);
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
