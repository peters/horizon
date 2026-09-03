use horizon_browser::{BrowserControlAction, BrowserControlValue};
use horizon_core::browser::manifest::AuditPageRequest;
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
    EvaluateInput, EvaluateOutput, HandoffInput, HandoffOutput, NavigateInput, NavigateOutput, NetworkInput,
    NetworkOutput, NetworkWatchInput, NetworkWatchOutput, NodeOutput, NodesOutput, PanelInput, QueryInput,
    SnapshotInput, SnapshotOutput, VisibilityInput, VisibilityOutput, WaitInput, WaitOutput,
};
use crate::network_watch::NetworkWatchState;

const DEFAULT_SNAPSHOT_NODES: u32 = 250;
const DEFAULT_QUERY_RESULTS: u32 = 50;

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
        description = "List live Horizon browser panels with their safe capabilities. For an agent identity injected by Horizon, only panels in that agent's current workspace are listed; identities from outside Horizon keep unscoped discovery. A panel's visible field is host presentation state, not workspace membership. Raw CDP, BiDi, and WebDriver endpoints are never exposed."
    )]
    fn browser_list(&self) -> Result<Json<BrowserListOutput>, String> {
        let panels = self.controller.list_panels().map_err(|error| error.to_string())?;
        Ok(Json(BrowserListOutput { panels }))
    }

    #[tool(
        name = "browser_create",
        description = "Create a browser panel in the calling agent's Horizon workspace and wait until its backend is ready and, when url is given, that page has committed; if the page has not committed within a bounded startup wait the panel is still returned with navigation=pending, and if the first page failed to load it is returned with navigation=failed and navigation_error (the panel is controllable; navigate again). Use this when browser_list is empty. Reuse an existing panel for iframes, popups, dialogs, and consent flows; never create a helper panel for them. Only when the user explicitly requests an independent session, set allow_additional=true. Set visible=false for a live background panel; omit backend to use Horizon's configured browser. Bare hostnames default to HTTPS and explicit HTTP is preserved."
    )]
    async fn browser_create(&self, Parameters(input): Parameters<CreateInput>) -> Result<Json<CreateOutput>, String> {
        // Matches the request normalization: a blank url is no navigation.
        let url_requested = input.url.as_deref().is_some_and(|url| !url.trim().is_empty());
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
            navigation: crate::model::CreateNavigationState::resolve(receipt.navigation, url_requested),
            navigation_error: receipt.navigation_error,
            startup_millis: receipt.startup_millis,
        }))
    }

    #[tool(
        name = "browser_visibility",
        description = "Show or hide a live browser panel without stopping its browser session, network capture, ownership, or MCP control. Hidden panels remain listed and auditable. For a Horizon-injected agent identity the panel must already be in the calling agent's workspace; visible=true never moves a panel there."
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
        description = "Navigate a live browser panel through Horizon's audited backend-neutral control path and report a typed outcome. By default it returns once the top-level document committed (wait=commit); wait=dispatched returns as soon as the engine handed the command to the backend, without awaiting the browser's acceptance (a rejection after that point shows as a failed page state, not as an action error; use commit when you need confirmation), and wait=dom_content_loaded after DOMContentLoaded. The result carries requested_url, committed_url, title, loading, redirected, elapsed_millis and state; check completed, because a wait that exceeds timeout_millis (1000-60000 ms, enforced in full by the engine; smaller values are raised to 1000) returns state=timed_out with the latest page state instead of an error. Unreachable destinations and rejected commands are errors. Safari's classic WebDriver has no dispatch-only navigation: every wait returns once the page loaded or the bound elapsed."
    )]
    async fn browser_navigate(
        &self,
        Parameters(input): Parameters<NavigateInput>,
    ) -> Result<Json<NavigateOutput>, String> {
        let wait = input.wait.unwrap_or_default();
        let receipt = self
            .controller
            .execute_engine_bounded(
                &input.panel_id,
                BrowserControlAction::Navigate {
                    url: input.url,
                    wait: wait.into(),
                    timeout_millis: Some(bounded_navigation_timeout(input.timeout_millis)),
                },
                bounded_navigation_timeout(input.timeout_millis),
            )
            .await
            .map_err(|error| error.to_string())?;
        match receipt.value {
            BrowserControlValue::Navigation { navigation } => Ok(Json(NavigateOutput::from_outcome(
                input.panel_id,
                receipt.action_id,
                navigation,
            ))),
            _ => Err("browser returned an unexpected navigation result".to_string()),
        }
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
        description = "Wait for a CSS selector to become present, visible, or hidden as one audited engine-side action: the browser driver observes the page itself (no repeated queries) and returns the matched nodes and elapsed_millis, or a typed failure (wait_timeout, wait_navigation_invalidated when the page navigated, wait_ownership_lost, wait_handoff_pending, wait_superseded when a later wait on the same panel replaced it, browser_unavailable when the browser backend stops). timeout_millis is 1000-60000 (default 10000) and is enforced in full by the engine; poll_millis is accepted for compatibility and ignored."
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
        description = "Read a bounded page of redacted audit entries for a browser panel. The default page is the newest matching records (limit 1-500, default 100). Set from_start=true to iterate from the oldest retained match; reuse next_event_id as after_event_id until has_more is false. Optional action_id filter. The response includes pagination and loss metadata: records_returned, records_retained, malformed_records, older_records_dropped, and cursor_lost when after_event_id is no longer retained."
    )]
    fn browser_audit(&self, Parameters(input): Parameters<AuditInput>) -> Result<Json<AuditOutput>, String> {
        let page = self
            .controller
            .read_audit_page(
                &input.panel_id,
                &AuditPageRequest::new(
                    input.after_event_id,
                    input.from_start.unwrap_or(false),
                    input.action_id,
                    input.limit,
                ),
            )
            .map_err(|error| error.to_string())?;
        let entries = page
            .entries
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not encode browser audit: {error}"))?;
        Ok(Json(AuditOutput {
            panel_id: input.panel_id,
            entries,
            next_event_id: page.next_event_id,
            has_more: page.has_more,
            records_returned: page.records_returned,
            records_retained: page.records_retained,
            malformed_records: page.malformed_records,
            older_records_dropped: page.older_records_dropped,
            cursor_lost: page.cursor_lost,
        }))
    }

    #[tool(
        name = "browser_panel",
        description = "Read one live Horizon browser panel's safe state. For a Horizon-injected agent identity, panel ids outside the calling agent's workspace are rejected. Use browser_list when the panel id is unknown."
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
                "MCP is the sole agent control contract for Horizon browser panels. For agent identities injected by Horizon, discovery and control are scoped to the workspace that contains the calling agent panel: browser_list shows only that workspace's panels, every other tool rejects panel ids outside it, and a panel's visible field is host presentation state rather than proof that it is in your workspace; identities from outside Horizon keep unscoped discovery. Start with browser_list; if it is empty, call browser_create in your current Horizon workspace. Reuse an existing panel for iframe, popup, dialog, and consent interactions: never create or reveal a helper panel as a workaround. If the current top-level semantic tools cannot reach embedded frame content, call browser_handoff on the original panel. Only set browser_create allow_additional=true when the user explicitly requests an independent browser session. Use visible=false for a live background panel and browser_visibility to show or hide it later without stopping automation or capture. Each panel advertises navigation, DOM, steering, audit, and backend-specific network capabilities. For WebSocket or HTTP observation, call browser_network start before browser_navigate; opt into native bounded response bodies with include_http_bodies. Prefer browser_network_watch with its returned capture_id and next_sequence for bounded event-driven monitoring, or tail the exact private NDJSON path returned by browser_network with ordinary read-only Unix tools; call browser_network stop to flush. Take a fresh semantic snapshot or query before acting through refs, and verify afterward. Never use raw browser endpoints or Horizon's private runtime files.",
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

/// Shortest navigation bound this server accepts: the engine reports its
/// typed `timed_out` outcome on a 250 ms coordination tick, so smaller
/// bounds could only end in this server's generic timeout.
const MIN_NAVIGATION_TIMEOUT_MILLIS: u64 = 1_000;

/// The bound the engine enforces for a navigation action; the controller
/// waits `RESULT_DELIVERY_HEADROOM_MILLIS` longer for the typed report.
fn bounded_navigation_timeout(timeout_millis: Option<u64>) -> u64 {
    crate::controller::bounded_action_timeout(timeout_millis).max(MIN_NAVIGATION_TIMEOUT_MILLIS)
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

/// Shortest wait bound this server accepts; the engine observes on a 100 ms
/// cadence and reports its typed timeout on a 250 ms coordination tick.
const MIN_WAIT_TIMEOUT_MILLIS: u64 = 1_000;

/// The wait this server applies to a `WaitForSelector` action.
fn bounded_wait_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(horizon_browser::DEFAULT_WAIT_TIMEOUT_MILLIS)
        .clamp(MIN_WAIT_TIMEOUT_MILLIS, MAX_ACTION_TIMEOUT_MILLIS)
}

/// One audited engine-side wait: the browser driver observes the selector
/// itself and reports the matched nodes, the elapsed time, or a typed
/// failure (`wait_timeout`, `wait_navigation_invalidated`,
/// `wait_ownership_lost`, `wait_handoff_pending`, `wait_superseded`,
/// `browser_unavailable`).
async fn wait_for_selector(controller: &BrowserController, input: WaitInput) -> Result<WaitOutput, String> {
    let timeout_millis = bounded_wait_timeout(input.timeout_millis);
    if let Some(poll_millis) = input.poll_millis {
        tracing::debug!(target: "browser_mcp", poll_millis, "browser_wait poll_millis is ignored; the engine observes the page itself");
    }
    let receipt = controller
        .execute_engine_bounded(
            &input.panel_id,
            BrowserControlAction::WaitForSelector {
                selector: input.selector,
                state: input.state.into(),
                timeout_millis: Some(timeout_millis),
            },
            timeout_millis,
        )
        .await
        .map_err(|error| error.to_string())?;
    let BrowserControlValue::Wait { wait } = receipt.value else {
        return Err("browser returned an unexpected wait result".to_string());
    };
    Ok(WaitOutput::from_outcome(input.panel_id, receipt.action_id, wait))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WaitState;

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
        assert!(schemas.contains("browser_unavailable"));
        assert!(schemas.contains("after_event_id"));
        assert!(schemas.contains("from_start"));
        assert!(schemas.contains("next_event_id"));
        assert!(schemas.contains("older_records_dropped"));
        assert!(schemas.contains("cursor_lost"));
        assert!(!schemas.contains("browser_ws"));
        assert!(!schemas.contains("manifest_path"));
        assert!(!schemas.contains("cdp_endpoint"));
    }

    #[test]
    fn navigation_outcomes_map_to_typed_tool_results() {
        let outcome = |state| horizon_browser::NavigationOutcome {
            requested_url: "https://example.test/".to_string(),
            wait: horizon_browser::NavigationWait::Commit,
            state,
            committed_url: Some("https://example.test/landing".to_string()),
            title: None,
            loading: true,
            redirected: true,
            elapsed_millis: 12,
        };
        let committed = NavigateOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            outcome(horizon_browser::NavigationState::Committed),
        );
        assert!(committed.completed);
        assert_eq!(committed.state, crate::model::NavigateState::Committed);
        assert_eq!(committed.wait, crate::model::NavigateWait::Commit);
        assert!(committed.redirected);
        assert_eq!(committed.committed_url.as_deref(), Some("https://example.test/landing"));
        for state in [
            horizon_browser::NavigationState::TimedOut,
            horizon_browser::NavigationState::Superseded,
        ] {
            let output = NavigateOutput::from_outcome("panel".to_string(), "action".to_string(), outcome(state));
            assert!(!output.completed, "{state:?} never counts as completed");
            assert_eq!(output.elapsed_millis, 12);
        }
        let dispatched = NavigateOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            outcome(horizon_browser::NavigationState::Dispatched),
        );
        assert!(dispatched.completed);

        assert_eq!(bounded_navigation_timeout(None), 15_000);
        assert_eq!(
            bounded_navigation_timeout(Some(100)),
            1_000,
            "bounds below 1 s are raised"
        );
        assert_eq!(
            bounded_navigation_timeout(Some(600_000)),
            60_000,
            "the engine gets the full documented bound; the controller adds delivery headroom"
        );
        let encoded = serde_json::to_value(crate::model::NavigateWait::DomContentLoaded).expect("encode");
        assert_eq!(encoded, serde_json::json!("dom_content_loaded"));
        let aliased: crate::model::NavigateWait =
            serde_json::from_value(serde_json::json!("domcontentloaded")).expect("alias decodes");
        assert_eq!(aliased, crate::model::NavigateWait::DomContentLoaded);
    }

    #[test]
    fn wait_inputs_map_to_one_bounded_engine_action() {
        assert_eq!(bounded_wait_timeout(None), 10_000);
        assert_eq!(bounded_wait_timeout(Some(10)), 1_000, "bounds below 1 s are raised");
        assert_eq!(bounded_wait_timeout(Some(600_000)), 60_000);
        let state: horizon_browser::SelectorState = WaitState::Hidden.into();
        assert_eq!(state, horizon_browser::SelectorState::Hidden);
        let output = WaitOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            horizon_browser::WaitOutcome {
                state: horizon_browser::SelectorState::Visible,
                generation: 2,
                revision: 3,
                nodes: Vec::new(),
                elapsed_millis: 1_480,
                polls: 15,
            },
        );
        assert_eq!(output.state, "visible");
        assert_eq!(output.elapsed_millis, 1_480);
        assert_eq!(output.polls, 15);
        assert_eq!((output.generation, output.revision), (2, 3));
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
