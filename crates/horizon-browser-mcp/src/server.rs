use std::time::{Duration, Instant};

use horizon_browser::{BrowserControlAction, BrowserControlValue};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::ToolCallContext,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool, tool_router,
};

use crate::controller::{BrowserController, MAX_ACTION_TIMEOUT_MILLIS};
use crate::model::{
    ActInput, ActKind, ActionOutput, AuditInput, AuditOutput, BrowserListOutput, EvaluateInput, EvaluateOutput,
    HandoffInput, HandoffOutput, NavigateInput, NodeOutput, NodesOutput, PanelInput, QueryInput, SnapshotInput,
    SnapshotOutput, WaitInput, WaitOutput, WaitState,
};

const DEFAULT_SNAPSHOT_NODES: u32 = 250;
const DEFAULT_QUERY_RESULTS: u32 = 50;
const DEFAULT_WAIT_TIMEOUT_MILLIS: u64 = 10_000;
const DEFAULT_WAIT_POLL_MILLIS: u64 = 250;

/// Stdio MCP service for live Horizon browser panels.
#[derive(Clone, Debug)]
pub struct HorizonBrowserMcp {
    controller: BrowserController,
    tool_router: ToolRouter<Self>,
}

impl HorizonBrowserMcp {
    /// Build a service using the injected Horizon panel identity when present.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::new(BrowserController::from_environment())
    }

    fn new(controller: BrowserController) -> Self {
        Self {
            controller,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl HorizonBrowserMcp {
    #[tool(
        name = "browser_list",
        description = "List live Horizon browser panels and safe capabilities. Raw CDP, BiDi, and WebDriver endpoints are never exposed."
    )]
    fn browser_list(&self) -> Json<BrowserListOutput> {
        Json(BrowserListOutput {
            panels: self.controller.list_panels(),
        })
    }

    #[tool(
        name = "browser_navigate",
        description = "Navigate a live browser panel through Horizon's audited backend-neutral control path."
    )]
    async fn browser_navigate(
        &self,
        Parameters(input): Parameters<NavigateInput>,
    ) -> Result<Json<ActionOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Navigate { url: input.url },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        require_accepted(&receipt.value)?;
        Ok(Json(ActionOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            completed: true,
        }))
    }

    #[tool(
        name = "browser_snapshot",
        description = "Return a bounded semantic snapshot with short-lived refs. Take a new snapshot after navigation or page changes."
    )]
    async fn browser_snapshot(
        &self,
        Parameters(input): Parameters<SnapshotInput>,
    ) -> Result<Json<SnapshotOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Snapshot {
                    max_nodes: input.max_nodes.unwrap_or(DEFAULT_SNAPSHOT_NODES),
                },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Snapshot { snapshot } = receipt.value else {
            return Err("browser returned an unexpected snapshot result".to_string());
        };
        Ok(Json(SnapshotOutput::new(input.panel_id, receipt.action_id, snapshot)))
    }

    #[tool(
        name = "browser_query",
        description = "Query the top-level document with a CSS selector and return matching nodes with short-lived refs."
    )]
    async fn browser_query(&self, Parameters(input): Parameters<QueryInput>) -> Result<Json<NodesOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Query {
                    selector: input.selector,
                    max_results: input.max_results.unwrap_or(DEFAULT_QUERY_RESULTS),
                },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Nodes {
            generation,
            revision,
            nodes,
        } = receipt.value
        else {
            return Err("browser returned an unexpected query result".to_string());
        };
        Ok(Json(NodesOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            generation,
            revision,
            nodes: nodes.into_iter().map(NodeOutput::from).collect(),
        }))
    }

    #[tool(
        name = "browser_act",
        description = "Click (including trusted double-click with count=2), fill, scroll, reload, go back, or go forward. Prefer a fresh ref from browser_snapshot or browser_query over a selector."
    )]
    async fn browser_act(&self, Parameters(input): Parameters<ActInput>) -> Result<Json<ActionOutput>, String> {
        let action = input.build_action()?;
        let action_kind = input.action;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        require_action_completed(action_kind, &receipt.value)?;
        Ok(Json(ActionOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            completed: true,
        }))
    }

    #[tool(
        name = "browser_evaluate",
        description = "Evaluate an explicit JavaScript expression in the top-level document. The expression is audited only by character count."
    )]
    async fn browser_evaluate(
        &self,
        Parameters(input): Parameters<EvaluateInput>,
    ) -> Result<Json<EvaluateOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Evaluate {
                    expression: input.expression,
                },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Json { value } = receipt.value else {
            return Err("browser returned an unexpected evaluation result".to_string());
        };
        Ok(Json(EvaluateOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            value,
        }))
    }

    #[tool(
        name = "browser_wait",
        description = "Wait for a CSS selector to become present, visible, or hidden using bounded audited queries."
    )]
    async fn browser_wait(&self, Parameters(input): Parameters<WaitInput>) -> Result<Json<WaitOutput>, String> {
        wait_for_selector(&self.controller, input).await.map(Json)
    }

    #[tool(
        name = "browser_handoff",
        description = "Pause agent actions and ask the user to steer the panel. Poll browser_list until handoff_pending is false before resuming."
    )]
    fn browser_handoff(&self, Parameters(input): Parameters<HandoffInput>) -> Result<Json<HandoffOutput>, String> {
        let request_id = self
            .controller
            .request_handoff(&input.panel_id, &input.reason)
            .map_err(|error| error.to_string())?;
        Ok(Json(HandoffOutput {
            panel_id: input.panel_id,
            request_id,
            handoff_pending: true,
        }))
    }

    #[tool(
        name = "browser_audit",
        description = "Read the newest redacted audit entries for a browser panel, optionally filtered by action id."
    )]
    fn browser_audit(&self, Parameters(input): Parameters<AuditInput>) -> Result<Json<AuditOutput>, String> {
        let limit = input.limit.unwrap_or(100).clamp(1, 500);
        let mut entries = self
            .controller
            .read_audit(&input.panel_id)
            .map_err(|error| error.to_string())?;
        if let Some(action_id) = input.action_id.as_deref() {
            entries.retain(|entry| entry.action_id == action_id);
        }
        let start = entries.len().saturating_sub(limit);
        let entries = entries
            .into_iter()
            .skip(start)
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not encode browser audit: {error}"))?;
        Ok(Json(AuditOutput {
            panel_id: input.panel_id,
            entries,
        }))
    }

    #[tool(
        name = "browser_panel",
        description = "Read one live Horizon browser panel's safe state. Use browser_list when the panel id is unknown."
    )]
    fn browser_panel(
        &self,
        Parameters(input): Parameters<PanelInput>,
    ) -> Result<Json<crate::model::BrowserPanel>, String> {
        self.controller
            .list_panels()
            .into_iter()
            .find(|panel| panel.panel_id == input.panel_id)
            .map(Json)
            .ok_or_else(|| format!("browser panel {} is not live", input.panel_id))
    }
}

impl ServerHandler for HorizonBrowserMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-browser", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "MCP is the sole agent control contract for Horizon browser panels. List panels, take a fresh semantic snapshot or query, act through refs, then verify. Never use raw browser endpoints.",
            )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

fn require_accepted(value: &BrowserControlValue) -> Result<(), String> {
    if matches!(value, BrowserControlValue::Accepted) {
        Ok(())
    } else {
        Err("browser returned an unexpected action result".to_string())
    }
}

fn require_action_completed(action: ActKind, value: &BrowserControlValue) -> Result<(), String> {
    if matches!(value, BrowserControlValue::Accepted)
        || matches!((action, value), (ActKind::Scroll, BrowserControlValue::Json { .. }))
    {
        Ok(())
    } else {
        Err("browser returned an unexpected action result".to_string())
    }
}

async fn wait_for_selector(controller: &BrowserController, input: WaitInput) -> Result<WaitOutput, String> {
    let timeout_millis = input
        .timeout_millis
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_MILLIS)
        .clamp(1, MAX_ACTION_TIMEOUT_MILLIS);
    let poll_millis = input.poll_millis.unwrap_or(DEFAULT_WAIT_POLL_MILLIS).clamp(100, 2_000);
    let started = Instant::now();
    loop {
        let remaining = timeout_millis.saturating_sub(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        if remaining == 0 {
            return Err(format!(
                "selector did not become {} within {timeout_millis} ms",
                wait_state_name(input.state)
            ));
        }
        let receipt = controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Query {
                    selector: input.selector.clone(),
                    max_results: DEFAULT_QUERY_RESULTS,
                },
                Some(remaining),
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Nodes { nodes, .. } = receipt.value else {
            return Err("browser returned an unexpected wait result".to_string());
        };
        if wait_satisfied(input.state, &nodes) {
            return Ok(WaitOutput {
                panel_id: input.panel_id,
                action_id: receipt.action_id,
                state: wait_state_name(input.state).to_string(),
                nodes: nodes.into_iter().map(NodeOutput::from).collect(),
            });
        }
        tokio::time::sleep(Duration::from_millis(poll_millis.min(remaining))).await;
    }
}

fn wait_satisfied(state: WaitState, nodes: &[horizon_browser::BrowserNode]) -> bool {
    match state {
        WaitState::Present => !nodes.is_empty(),
        WaitState::Visible => nodes.iter().any(|node| node.visible),
        WaitState::Hidden => nodes.iter().all(|node| !node.visible),
    }
}

const fn wait_state_name(state: WaitState) -> &'static str {
    match state {
        WaitState::Present => "present",
        WaitState::Visible => "visible",
        WaitState::Hidden => "hidden",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_exposes_only_the_mcp_contract() {
        let server = HorizonBrowserMcp::new(BrowserController::with_actor("agent"));
        let mut names = server
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "browser_act",
                "browser_audit",
                "browser_evaluate",
                "browser_handoff",
                "browser_list",
                "browser_navigate",
                "browser_panel",
                "browser_query",
                "browser_snapshot",
                "browser_wait",
            ]
        );
        let schemas = serde_json::to_string(&server.tool_router.list_all()).unwrap_or_default();
        assert!(!schemas.contains("browser_ws"));
        assert!(!schemas.contains("manifest_path"));
        assert!(!schemas.contains("cdp_endpoint"));
    }

    #[test]
    fn wait_states_are_exact() {
        let visible = horizon_browser::BrowserNode {
            reference: "g1s1e1".to_string(),
            role: "button".to_string(),
            name: "Continue".to_string(),
            text: String::new(),
            visible: true,
            enabled: true,
            bounds: None,
        };
        assert!(wait_satisfied(WaitState::Present, std::slice::from_ref(&visible)));
        assert!(wait_satisfied(WaitState::Visible, std::slice::from_ref(&visible)));
        assert!(!wait_satisfied(WaitState::Hidden, &[visible]));
        assert!(wait_satisfied(WaitState::Hidden, &[]));
    }

    #[test]
    fn scroll_accepts_the_engines_scroll_state_completion() {
        assert!(require_action_completed(ActKind::Click, &BrowserControlValue::Accepted).is_ok());
        assert!(
            require_action_completed(
                ActKind::Scroll,
                &BrowserControlValue::Json {
                    value: serde_json::json!({ "scrollY": 480 }),
                },
            )
            .is_ok()
        );
        assert!(
            require_action_completed(
                ActKind::Click,
                &BrowserControlValue::Json {
                    value: serde_json::Value::Null,
                },
            )
            .is_err()
        );
    }
}
