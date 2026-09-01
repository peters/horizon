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
    ActInput, ActKind, ActionOutput, AuditInput, AuditOutput, BrowserListOutput, CreateInput, CreateOutput,
    EvaluateInput, EvaluateOutput, HandoffInput, HandoffOutput, NavigateInput, NetworkInput, NetworkOutput,
    NetworkWatchInput, NetworkWatchOutput, NodeOutput, NodesOutput, PanelInput, QueryInput, SnapshotInput,
    SnapshotOutput, VisibilityInput, VisibilityOutput, WaitInput, WaitOutput, WaitState,
};
use crate::network_watch::NetworkWatchState;

const DEFAULT_SNAPSHOT_NODES: u32 = 250;
const DEFAULT_QUERY_RESULTS: u32 = 50;
const DEFAULT_WAIT_TIMEOUT_MILLIS: u64 = 10_000;
const DEFAULT_WAIT_POLL_MILLIS: u64 = 250;

/// Stdio MCP service for live Horizon browser panels.
#[derive(Clone, Debug)]
pub struct HorizonBrowserMcp {
    controller: BrowserController,
    network_watch: NetworkWatchState,
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
            network_watch: NetworkWatchState::default(),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl HorizonBrowserMcp {
    #[tool(
        name = "browser_list",
        description = "List the live Horizon browser panels in the calling agent's workspace with their safe capabilities. Panels in other workspaces are never listed, and a panel's visible field is host presentation state, not workspace membership. Raw CDP, BiDi, and WebDriver endpoints are never exposed."
    )]
    fn browser_list(&self) -> Json<BrowserListOutput> {
        Json(BrowserListOutput {
            panels: self.controller.list_panels(),
        })
    }

    #[tool(
        name = "browser_create",
        description = "Create a browser panel in the calling agent's Horizon workspace and wait until it is controllable. Use this when browser_list is empty. Reuse an existing panel for iframes, popups, dialogs, and consent flows; never create a helper panel for them. Only when the user explicitly requests an independent session, set allow_additional=true. Set visible=false for a live background panel; omit backend to use Horizon's configured browser. Bare hostnames default to HTTPS and explicit HTTP is preserved."
    )]
    async fn browser_create(&self, Parameters(input): Parameters<CreateInput>) -> Result<Json<CreateOutput>, String> {
        let receipt = self
            .controller
            .create(
                input.url,
                input.backend.map(Into::into),
                input.visible.unwrap_or(true),
                input.allow_additional.unwrap_or(false),
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(CreateOutput {
            action_id: receipt.action_id,
            panel: receipt.panel,
        }))
    }

    #[tool(
        name = "browser_visibility",
        description = "Show or hide a live browser panel without stopping its browser session, network capture, ownership, or MCP control. Hidden panels remain listed and auditable. The panel must already be in the calling agent's workspace; visible=true never moves a panel there."
    )]
    async fn browser_visibility(
        &self,
        Parameters(input): Parameters<VisibilityInput>,
    ) -> Result<Json<VisibilityOutput>, String> {
        let receipt = self
            .controller
            .set_visibility(&input.panel_id, input.visible, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(VisibilityOutput {
            action_id: receipt.action_id,
            panel: receipt.panel,
        }))
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
        name = "browser_network",
        description = "Start, inspect, or stop a bounded browser network capture. Start before navigation; set include_http_bodies for native bounded response bodies. Inspect status or tail -f the returned private NDJSON path with jq/rg. Chromium WebSocket frames are protocol-native; Firefox WebSocket frames use disclosed page instrumentation; HTTP lifecycle and bodies are native on both. Chromium cannot return a body the page drained with response.blob(); such http_response_body records carry an error and no payload, so read text()/arrayBuffer() or leave the body unread when the bytes matter. Safari is unsupported."
    )]
    async fn browser_network(
        &self,
        Parameters(input): Parameters<NetworkInput>,
    ) -> Result<Json<NetworkOutput>, String> {
        let action = input.build_action()?;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Network { capture } = receipt.value else {
            return Err("browser returned an unexpected network capture result".to_string());
        };
        Ok(Json(NetworkOutput::new(input.panel_id, receipt.action_id, capture)))
    }

    #[tool(
        name = "browser_network_watch",
        description = "Wait up to 60 seconds for a bounded batch of matching records from the active Horizon-owned network capture. Resume with capture_id and next_sequence; filters run before records are returned, payloads are excluded by default, and no capture path is accepted or exposed."
    )]
    async fn browser_network_watch(
        &self,
        Parameters(input): Parameters<NetworkWatchInput>,
    ) -> Result<Json<NetworkWatchOutput>, String> {
        self.network_watch.watch(&self.controller, input).await.map(Json)
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
        description = "Read one live Horizon browser panel's safe state. Panel ids outside the calling agent's workspace are rejected; use browser_list when the panel id is unknown."
    )]
    fn browser_panel(
        &self,
        Parameters(input): Parameters<PanelInput>,
    ) -> Result<Json<crate::model::BrowserPanel>, String> {
        self.controller
            .panel(&input.panel_id)
            .map(Json)
            .map_err(|error| error.to_string())
    }
}

impl ServerHandler for HorizonBrowserMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-browser", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "MCP is the sole agent control contract for Horizon browser panels. Discovery and control are scoped to the workspace that contains the calling agent panel: browser_list shows only that workspace's panels, every other tool rejects panel ids outside it, and a panel's visible field is host presentation state rather than proof that it is in your workspace. Start with browser_list; if it is empty, call browser_create in your current Horizon workspace. Reuse an existing panel for iframe, popup, dialog, and consent interactions: never create or reveal a helper panel as a workaround. If the current top-level semantic tools cannot reach embedded frame content, call browser_handoff on the original panel. Only set browser_create allow_additional=true when the user explicitly requests an independent browser session. Use visible=false for a live background panel and browser_visibility to show or hide it later without stopping automation or capture. Each panel advertises navigation, DOM, steering, audit, and backend-specific network capabilities. For WebSocket or HTTP observation, call browser_network start before browser_navigate; opt into native bounded response bodies with include_http_bodies. Prefer browser_network_watch with its returned capture_id and next_sequence for bounded event-driven monitoring, or tail the exact private NDJSON path returned by browser_network with ordinary read-only Unix tools; call browser_network stop to flush. Take a fresh semantic snapshot or query before acting through refs, and verify afterward. Never use raw browser endpoints or Horizon's private runtime files.",
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
                "browser_create",
                "browser_evaluate",
                "browser_handoff",
                "browser_list",
                "browser_navigate",
                "browser_network",
                "browser_network_watch",
                "browser_panel",
                "browser_query",
                "browser_snapshot",
                "browser_visibility",
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
