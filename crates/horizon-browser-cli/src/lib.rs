#![forbid(unsafe_code)]

//! Scriptable command-line control for live Horizon browser panels.
//!
//! Plans contain literal calls to the existing MCP tools. This crate does not
//! add a second browser action API: it connects an MCP client to the same
//! [`horizon_browser_mcp::HorizonBrowserMcp`] service used by agents.

pub mod execution_control;
pub mod job;
pub mod observability;
pub mod run_state;
pub mod standalone;

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::{Future, poll_fn};
use std::time::Duration;

use rmcp::{
    ClientHandler, ServiceExt as _,
    model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use execution_control::{ExecutionControl, ExecutionStopReason};
use observability::ObservabilitySummary;

const PLAN_VERSION: u32 = 1;
const MAX_PLAN_STEPS: usize = 256;
const MCP_BUFFER_BYTES: usize = 64 * 1024;
const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// A versioned, fail-fast sequence of Horizon browser MCP tool calls.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Plan format version. The only supported value is `1`.
    pub version: u32,
    /// Tool calls in execution order.
    pub steps: Vec<PlanStep>,
}

/// One named MCP tool call in a [`Plan`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    /// Unique identifier used in the report and by later `$ref` values.
    pub id: String,
    /// Exact MCP tool name, such as `browser_list` or `browser_navigate`.
    pub tool: String,
    /// MCP tool arguments. An exact `{"$ref":"step#/pointer"}` object is
    /// replaced with a prior step's typed structured result value.
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

/// Machine-readable result of executing a plan.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExecutionReport {
    /// Report format version.
    pub version: u32,
    /// True only when every step and MCP shutdown completed successfully.
    pub ok: bool,
    /// Number of step reports emitted before success or fail-fast stop.
    pub completed_steps: usize,
    /// Ordered results. Tool arguments are deliberately excluded.
    pub steps: Vec<StepReport>,
    /// Connection-level failure or execution-deadline description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Explicit cancellation or deadline stop condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<ExecutionStopReason>,
    /// Audit completeness and network-capture health from executed MCP results.
    pub observability: ObservabilitySummary,
}

/// Result of one MCP tool call.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepReport {
    /// Plan step identifier.
    pub id: String,
    /// Exact MCP tool called.
    pub tool: String,
    /// True when MCP reported a successful structured result.
    pub ok: bool,
    /// MCP structured content, omitted when none was returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Public MCP error text, omitted on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A plan is invalid before any browser action is attempted.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    /// The plan JSON could not be decoded.
    #[error("invalid plan JSON: {0}")]
    Json(String),
    /// The plan version is not supported.
    #[error("unsupported plan version {0}; expected 1")]
    Version(u32),
    /// The plan has no work.
    #[error("plan must contain at least one step")]
    Empty,
    /// The plan exceeds the bounded step count.
    #[error("plan has {actual} steps; the maximum is {maximum}")]
    TooManySteps { actual: usize, maximum: usize },
    /// A step identifier is malformed.
    #[error("invalid step id `{0}`; use 1-64 ASCII letters, digits, `_`, or `-`")]
    InvalidStepId(String),
    /// A step identifier was reused.
    #[error("duplicate step id `{0}`")]
    DuplicateStepId(String),
    /// A tool name is malformed.
    #[error("step `{step}` has invalid MCP tool name `{tool}`")]
    InvalidToolName { step: String, tool: String },
    /// A `$ref` value is malformed or ambiguous.
    #[error("step `{step}` has invalid reference: {reason}")]
    InvalidReference { step: String, reason: String },
    /// A `$ref` names a step that is not earlier in the plan.
    #[error("step `{step}` references unavailable prior step `{target}`")]
    UnavailableReference { step: String, target: String },
}

/// Failure to initialize or use the in-process MCP connection.
#[derive(Debug, Error)]
pub enum RunError {
    /// The plan failed validation.
    #[error(transparent)]
    Plan(#[from] PlanError),
    /// The MCP client/server session could not start.
    #[error("could not start Horizon browser MCP session: {0}")]
    Initialize(String),
    /// A plan names a tool the current MCP server does not publish.
    #[error("plan step `{step}` names unavailable MCP tool `{tool}`")]
    UnknownTool { step: String, tool: String },
    /// The job stopped before step execution produced a report.
    #[error("{}", .0.message())]
    Stopped(ExecutionStopReason),
}

#[derive(Clone, Debug, Default)]
struct PlanClient;

#[derive(Debug)]
struct ActionWaitStopped {
    reason: ExecutionStopReason,
    request_started: bool,
}

impl ClientHandler for PlanClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("horizon-browser-cli", env!("CARGO_PKG_VERSION")),
        )
    }
}

impl Plan {
    /// Decode and validate a plan without executing any actions.
    ///
    /// # Errors
    /// Returns a typed validation failure for malformed JSON or plan content.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, PlanError> {
        let plan = serde_json::from_slice(bytes).map_err(|error| PlanError::Json(error.to_string()))?;
        validate_plan(&plan)?;
        Ok(plan)
    }
}

/// Execute a plan through an in-process client/server MCP transport.
///
/// Tool calls are validated against `tools/list` before the first action. Tool
/// failures are returned as a fail-fast [`ExecutionReport`], not as a second
/// action-specific error model.
///
/// # Errors
/// Returns only for plan validation, MCP initialization, or unavailable tools.
pub async fn execute_plan(plan: &Plan) -> Result<ExecutionReport, RunError> {
    execute_plan_with_control(plan, &mut ExecutionControl::unbounded()).await
}

/// Execute a plan with shared cancellation and a job deadline.
///
/// A stop during tool execution returns a partial report containing only
/// completed calls. A stop during initialization returns [`RunError::Stopped`]
/// because no step report exists yet.
///
/// # Errors
/// Returns for plan validation, MCP initialization, unavailable tools, or a
/// stop before step execution begins.
pub async fn execute_plan_with_control(
    plan: &Plan,
    control: &mut ExecutionControl,
) -> Result<ExecutionReport, RunError> {
    validate_plan(plan)?;
    control.check().map_err(RunError::Stopped)?;
    let (server_transport, client_transport) = tokio::io::duplex(MCP_BUFFER_BYTES);
    let mut server_task = tokio::spawn(async move {
        let service = horizon_browser_mcp::HorizonBrowserMcp::from_environment()
            .serve(server_transport)
            .await
            .map_err(|error| error.to_string())?;
        service.waiting().await.map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    });
    let mut client = match control.wait(PlanClient.serve(client_transport)).await {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            server_task.abort();
            return Err(RunError::Initialize(error.to_string()));
        }
        Err(reason) => {
            server_task.abort();
            return Err(RunError::Stopped(reason));
        }
    };

    let tools = match control.wait(client.list_tools(None)).await {
        Ok(Ok(tools)) => tools,
        Ok(Err(error)) => {
            drop(client);
            server_task.abort();
            return Err(RunError::Initialize(error.to_string()));
        }
        Err(reason) => {
            drop(client);
            server_task.abort();
            return Err(RunError::Stopped(reason));
        }
    }
    .tools
    .into_iter()
    .map(|tool| tool.name.into_owned())
    .collect::<BTreeSet<_>>();
    for step in &plan.steps {
        if !tools.contains(&step.tool) {
            drop(client);
            server_task.abort();
            return Err(RunError::UnknownTool {
                step: step.id.clone(),
                tool: step.tool.clone(),
            });
        }
    }

    let mut report = execute_steps(plan, &client, control).await;
    if report.stop_reason.is_some() {
        drop(client);
        server_task.abort();
        return Ok(report);
    }
    match control.wait(client.close()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => report.fail_connection(format!("MCP client shutdown failed: {error}")),
        Err(reason) => {
            report.stop(reason);
            server_task.abort();
            return Ok(report);
        }
    }
    match control
        .wait(tokio::time::timeout(MCP_SHUTDOWN_TIMEOUT, &mut server_task))
        .await
    {
        Ok(Ok(Ok(Ok(())))) => {}
        Ok(Ok(Ok(Err(error)))) => report.fail_connection(format!("MCP server shutdown failed: {error}")),
        Ok(Ok(Err(error))) => report.fail_connection(format!("MCP server task failed: {error}")),
        Ok(Err(_)) => {
            server_task.abort();
            report.fail_connection("MCP server did not stop within 5 seconds".to_string());
        }
        Err(reason) => {
            server_task.abort();
            report.stop(reason);
        }
    }
    Ok(report)
}

async fn execute_steps(
    plan: &Plan,
    client: &rmcp::service::RunningService<rmcp::RoleClient, PlanClient>,
    control: &mut ExecutionControl,
) -> ExecutionReport {
    let mut result_indexes = BTreeMap::new();
    let mut steps = Vec::with_capacity(plan.steps.len());
    for step in &plan.steps {
        let arguments = match resolve_arguments(step, &steps, &result_indexes) {
            Ok(arguments) => arguments,
            Err(error) => {
                steps.push(failed_step(step, error));
                break;
            }
        };
        let params = CallToolRequestParams::new(step.tool.clone()).with_arguments(arguments);
        let result = match wait_for_browser_action(control, client.call_tool(params)).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                steps.push(failed_step(step, format!("MCP call failed: {error}")));
                break;
            }
            Err(stopped) => return stopped_report(steps, &stopped),
        };
        let structured = result.structured_content;
        if result.is_error.unwrap_or(false) {
            let error = tool_error_text(&result.content);
            steps.push(StepReport {
                id: step.id.clone(),
                tool: step.tool.clone(),
                ok: false,
                result: structured,
                error: Some(error),
            });
            break;
        }
        let Some(structured) = structured else {
            steps.push(failed_step(step, "MCP tool returned no structured content".to_string()));
            break;
        };
        steps.push(StepReport {
            id: step.id.clone(),
            tool: step.tool.clone(),
            ok: true,
            result: Some(structured),
            error: None,
        });
        result_indexes.insert(step.id.clone(), steps.len() - 1);
    }
    let ok = steps.len() == plan.steps.len() && steps.iter().all(|step| step.ok);
    execution_report(ok, steps, None, None)
}

async fn wait_for_browser_action<T>(
    control: &mut ExecutionControl,
    action: impl Future<Output = T>,
) -> Result<T, ActionWaitStopped> {
    let request_started = Cell::new(false);
    let result = {
        tokio::pin!(action);
        control
            .wait(poll_fn(|context| {
                // RMCP async calls cannot submit until their future is first
                // polled, so a ready deadline can remain phase-neutral.
                request_started.set(true);
                action.as_mut().poll(context)
            }))
            .await
    };
    result.map_err(|reason| ActionWaitStopped {
        reason,
        request_started: request_started.get(),
    })
}

impl ExecutionReport {
    fn fail_connection(&mut self, error: String) {
        self.ok = false;
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    fn stop(&mut self, reason: ExecutionStopReason) {
        self.ok = false;
        self.error = Some(reason.message().to_string());
        self.stop_reason = Some(reason);
    }
}

fn stopped_report(steps: Vec<StepReport>, stopped: &ActionWaitStopped) -> ExecutionReport {
    let message = if stopped.request_started {
        stopped.reason.in_flight_message()
    } else {
        stopped.reason.message()
    };
    execution_report(false, steps, Some(message.to_string()), Some(stopped.reason))
}

fn execution_report(
    ok: bool,
    steps: Vec<StepReport>,
    error: Option<String>,
    stop_reason: Option<ExecutionStopReason>,
) -> ExecutionReport {
    let observability = ObservabilitySummary::from_results(
        steps
            .iter()
            .filter_map(|step| step.result.as_ref().map(|result| (step.tool.as_str(), result))),
    );
    ExecutionReport {
        version: PLAN_VERSION,
        ok,
        completed_steps: steps.len(),
        steps,
        error,
        stop_reason,
        observability,
    }
}

fn failed_step(step: &PlanStep, error: String) -> StepReport {
    StepReport {
        id: step.id.clone(),
        tool: step.tool.clone(),
        ok: false,
        result: None,
        error: Some(error),
    }
}

fn tool_error_text(content: &[rmcp::model::ContentBlock]) -> String {
    let messages = content
        .iter()
        .filter_map(rmcp::model::ContentBlock::as_text)
        .map(|text| text.text.as_str())
        .collect::<Vec<_>>();
    if messages.is_empty() {
        "MCP tool returned an error".to_string()
    } else {
        messages.join("\n")
    }
}

fn validate_plan(plan: &Plan) -> Result<(), PlanError> {
    if plan.version != PLAN_VERSION {
        return Err(PlanError::Version(plan.version));
    }
    if plan.steps.is_empty() {
        return Err(PlanError::Empty);
    }
    if plan.steps.len() > MAX_PLAN_STEPS {
        return Err(PlanError::TooManySteps {
            actual: plan.steps.len(),
            maximum: MAX_PLAN_STEPS,
        });
    }
    let mut prior = BTreeSet::new();
    for step in &plan.steps {
        if !valid_identifier(&step.id) {
            return Err(PlanError::InvalidStepId(step.id.clone()));
        }
        if prior.contains(&step.id) {
            return Err(PlanError::DuplicateStepId(step.id.clone()));
        }
        if !valid_tool_name(&step.tool) {
            return Err(PlanError::InvalidToolName {
                step: step.id.clone(),
                tool: step.tool.clone(),
            });
        }
        validate_references(&Value::Object(step.arguments.clone()), step, &prior)?;
        prior.insert(step.id.clone());
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn validate_references(value: &Value, step: &PlanStep, prior: &BTreeSet<String>) -> Result<(), PlanError> {
    match value {
        Value::Array(values) => {
            for value in values {
                validate_references(value, step, prior)?;
            }
        }
        Value::Object(object) if object.contains_key("$ref") => {
            if object.len() != 1 {
                return Err(invalid_reference(step, "a $ref object cannot contain other fields"));
            }
            let reference = object
                .get("$ref")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_reference(step, "$ref must be a string"))?;
            let (target, pointer) = parse_reference(step, reference)?;
            if !prior.contains(target) {
                return Err(PlanError::UnavailableReference {
                    step: step.id.clone(),
                    target: target.to_string(),
                });
            }
            if !pointer.is_empty() && !pointer.starts_with('/') {
                return Err(invalid_reference(step, "JSON pointer must be empty or start with `/`"));
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                validate_references(value, step, prior)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_arguments(
    step: &PlanStep,
    results: &[StepReport],
    result_indexes: &BTreeMap<String, usize>,
) -> Result<Map<String, Value>, String> {
    let resolved = resolve_value(&Value::Object(step.arguments.clone()), step, results, result_indexes)?;
    resolved
        .as_object()
        .cloned()
        .ok_or_else(|| "resolved MCP arguments were not an object".to_string())
}

fn resolve_value(
    value: &Value,
    step: &PlanStep,
    results: &[StepReport],
    result_indexes: &BTreeMap<String, usize>,
) -> Result<Value, String> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| resolve_value(value, step, results, result_indexes))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            let reference = object
                .get("$ref")
                .and_then(Value::as_str)
                .ok_or_else(|| "$ref must be a string".to_string())?;
            let (target, pointer) = parse_reference(step, reference).map_err(|error| error.to_string())?;
            let result = result_indexes
                .get(target)
                .and_then(|index| results.get(*index))
                .and_then(|report| report.result.as_ref())
                .ok_or_else(|| format!("reference target `{target}` has no successful result"))?;
            result
                .pointer(pointer)
                .cloned()
                .ok_or_else(|| format!("reference `{reference}` did not match the prior structured result"))
        }
        Value::Object(object) => object
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_value(value, step, results, result_indexes)?)))
            .collect::<Result<Map<_, _>, String>>()
            .map(Value::Object),
        _ => Ok(value.clone()),
    }
}

fn parse_reference<'a>(step: &PlanStep, reference: &'a str) -> Result<(&'a str, &'a str), PlanError> {
    let (target, pointer) = reference
        .split_once('#')
        .ok_or_else(|| invalid_reference(step, "$ref must use `step-id#/json/pointer`"))?;
    if !valid_identifier(target) {
        return Err(invalid_reference(step, "reference step id is invalid"));
    }
    Ok((target, pointer))
}

fn invalid_reference(step: &PlanStep, reason: &str) -> PlanError {
    PlanError::InvalidReference {
        step: step.id.clone(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plan(bytes: &[u8]) -> Plan {
        Plan::from_slice(bytes).expect("valid plan")
    }

    #[test]
    fn plans_reject_typos_duplicates_and_forward_references() {
        assert!(matches!(
            Plan::from_slice(br#"{"version":1,"steps":[],"typo":true}"#),
            Err(PlanError::Json(_))
        ));
        assert!(matches!(
            Plan::from_slice(
                br#"{"version":1,"steps":[{"id":"same","tool":"browser_list"},{"id":"same","tool":"browser_list"}]}"#
            ),
            Err(PlanError::DuplicateStepId(id)) if id == "same"
        ));
        assert!(matches!(
            Plan::from_slice(
                br#"{"version":1,"steps":[{"id":"first","tool":"browser_panel","arguments":{"panel_id":{"$ref":"later#/panel_id"}}},{"id":"later","tool":"browser_list"}]}"#
            ),
            Err(PlanError::UnavailableReference { target, .. }) if target == "later"
        ));
    }

    #[test]
    fn references_preserve_json_types_and_support_pointer_escaping() {
        let plan = plan(
            br#"{"version":1,"steps":[{"id":"list","tool":"browser_list"},{"id":"next","tool":"browser_panel","arguments":{"panel_id":{"$ref":"list#/nested/panel~1id"},"visible":{"$ref":"list#/visible"}}}]}"#,
        );
        let results = [successful_step(
            "list",
            json!({"nested":{"panel/id":"panel-7"},"visible":true}),
        )];
        let indexes = BTreeMap::from([("list".to_string(), 0)]);
        let resolved = resolve_arguments(&plan.steps[1], &results, &indexes).expect("resolve arguments");
        assert_eq!(resolved["panel_id"], "panel-7");
        assert_eq!(resolved["visible"], true);
    }

    #[test]
    fn missing_pointer_is_a_bounded_step_failure() {
        let plan = plan(
            br#"{"version":1,"steps":[{"id":"list","tool":"browser_list"},{"id":"next","tool":"browser_panel","arguments":{"panel_id":{"$ref":"list#/missing"}}}]}"#,
        );
        let results = [successful_step("list", json!({"panels":[]}))];
        let indexes = BTreeMap::from([("list".to_string(), 0)]);
        let error = resolve_arguments(&plan.steps[1], &results, &indexes).expect_err("missing pointer");
        assert!(error.contains("did not match"));
    }

    #[tokio::test]
    async fn expired_control_stops_before_mcp_initialization() {
        let plan = plan(br#"{"version":1,"steps":[{"id":"list","tool":"browser_list"}]}"#);
        let mut control = ExecutionControl::with_timeout(Duration::ZERO);

        let error = execute_plan_with_control(&plan, &mut control)
            .await
            .expect_err("expired control must stop before MCP initialization");

        assert!(matches!(
            &error,
            RunError::Stopped(ExecutionStopReason::DeadlineExceeded)
        ));
        let message = error.to_string();
        assert_eq!(message, ExecutionStopReason::DeadlineExceeded.message());
        assert!(!message.contains("in-flight browser action"));
    }

    #[tokio::test]
    async fn ready_deadline_does_not_mark_an_unpolled_action_in_flight() {
        let mut control = ExecutionControl::with_timeout(Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(1)).await;

        let stopped = wait_for_browser_action(&mut control, std::future::pending::<()>())
            .await
            .expect_err("ready deadline must win before polling the action");
        assert!(!stopped.request_started);
        let report = stopped_report(Vec::new(), &stopped);

        assert_eq!(
            report.error.as_deref(),
            Some(ExecutionStopReason::DeadlineExceeded.message())
        );
        assert_eq!(report.stop_reason, Some(ExecutionStopReason::DeadlineExceeded));
    }

    fn successful_step(id: &str, result: Value) -> StepReport {
        StepReport {
            id: id.to_string(),
            tool: "browser_list".to_string(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }
}
