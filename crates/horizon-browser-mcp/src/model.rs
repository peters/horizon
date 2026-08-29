mod network;

pub(crate) use network::{
    NetworkInput, NetworkOutput, NetworkWatchCaptureState, NetworkWatchDeliveryState, NetworkWatchEventKind,
    NetworkWatchInput, NetworkWatchOutput, NetworkWatchRecord,
};

use horizon_browser::{BackendKind, BrowserBounds, BrowserNode, BrowserSnapshot, BrowserTarget};
use horizon_core::browser::manifest::{self, BrowserManifest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProtocolKind {
    Cdp,
    WebDriverBidi,
    WebDriver,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct BrowserPanel {
    pub(crate) panel_id: String,
    pub(crate) backend: String,
    pub(crate) protocol: ProtocolKind,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) visible: bool,
    pub(crate) owner: Option<String>,
    #[serde(flatten)]
    pub(crate) agent_state: BrowserPanelAgentState,
    pub(crate) capabilities: Vec<String>,
    pub(crate) network_capture: NetworkCaptureCapability,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct BrowserPanelAgentState {
    pub(crate) owned_by_caller: bool,
    pub(crate) user_active: bool,
    pub(crate) handoff_pending: bool,
}

impl BrowserPanel {
    pub(crate) fn from_manifest(value: BrowserManifest, actor: &str) -> Self {
        let now = manifest::now_millis();
        let owner = value.live_owner(now).map(|owner| owner.name.clone());
        let owned_by_caller = owner.as_deref() == Some(actor);
        let protocol = crate::controller::protocol_kind(value.backend, !value.browser_ws.is_empty());
        let user_active = value.user_is_active(now);
        let handoff_pending = value.handoff_pending().is_some();
        Self {
            panel_id: value.panel_local_id,
            backend: backend_name(value.backend).to_string(),
            protocol,
            url: value.url,
            title: value.title,
            visible: !value.hidden,
            owner,
            agent_state: BrowserPanelAgentState {
                owned_by_caller,
                user_active,
                handoff_pending,
            },
            capabilities: semantic_capabilities(value.backend),
            network_capture: NetworkCaptureCapability::for_backend(value.backend),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NetworkCaptureCapability {
    pub(crate) supported: bool,
    pub(crate) transport: Option<String>,
    pub(crate) websocket_frames: bool,
    pub(crate) http_response_body_transport: Option<String>,
    pub(crate) page_instrumentation: bool,
    pub(crate) workflow: String,
}

impl NetworkCaptureCapability {
    fn for_backend(backend: BackendKind) -> Self {
        let workflow = if backend == BackendKind::SafariWebDriver {
            "Network capture is unavailable for this backend.".to_string()
        } else {
            "Call browser_network start before navigation; opt into native bounded HTTP bodies with include_http_bodies, consume filtered records with browser_network_watch and its sequence cursor (or tail the returned private NDJSON path), then call stop to flush.".to_string()
        };
        match backend {
            BackendKind::ChromiumCdp => Self {
                supported: true,
                transport: Some("cdp".to_string()),
                websocket_frames: true,
                http_response_body_transport: Some("cdp".to_string()),
                page_instrumentation: false,
                workflow,
            },
            BackendKind::FirefoxBidi => Self {
                supported: true,
                transport: Some("webdriver_bidi_page_instrumentation".to_string()),
                websocket_frames: true,
                http_response_body_transport: Some("webdriver_bidi".to_string()),
                page_instrumentation: true,
                workflow,
            },
            BackendKind::SafariWebDriver => Self {
                supported: false,
                transport: None,
                websocket_frames: false,
                http_response_body_transport: None,
                page_instrumentation: false,
                workflow,
            },
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct BrowserListOutput {
    pub(crate) panels: Vec<BrowserPanel>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CreateBackend {
    Chromium,
    Firefox,
    Safari,
}

impl From<CreateBackend> for BackendKind {
    fn from(value: CreateBackend) -> Self {
        match value {
            CreateBackend::Chromium => Self::ChromiumCdp,
            CreateBackend::Firefox => Self::FirefoxBidi,
            CreateBackend::Safari => Self::SafariWebDriver,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateInput {
    /// Optional first page. Bare hostnames default to HTTPS; explicit HTTP is preserved.
    pub(crate) url: Option<String>,
    /// Browser override. Omit this to use Horizon's configured browser backend.
    pub(crate) backend: Option<CreateBackend>,
    /// Whether the panel is shown initially (default true). Hidden panels remain live and controllable.
    pub(crate) visible: Option<bool>,
    /// Explicitly permit another panel when this agent already owns one. Use only for a user-requested independent session.
    pub(crate) allow_additional: Option<bool>,
    /// Total host-and-browser startup timeout in milliseconds (5000-60000, default 60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct CreateOutput {
    /// Auditable identity for the panel creation lifecycle.
    pub(crate) action_id: String,
    /// Ready panel state. Use `panel.panel_id` with all other browser tools.
    pub(crate) panel: BrowserPanel,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct VisibilityInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Show (`true`) or hide (`false`) the panel without stopping its browser session.
    pub(crate) visible: bool,
    /// Host coordination timeout in milliseconds (1-60000, default 15000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct VisibilityOutput {
    pub(crate) action_id: String,
    pub(crate) panel: BrowserPanel,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PanelInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct NavigateInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Destination URL.
    pub(crate) url: String,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SnapshotInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Maximum semantic nodes to return (1-1000, default 250).
    pub(crate) max_nodes: Option<u32>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct QueryInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// CSS selector evaluated in the top-level document.
    pub(crate) selector: String,
    /// Maximum matching nodes to return (1-250, default 50).
    pub(crate) max_results: Option<u32>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActKind {
    Click,
    Fill,
    Scroll,
    Reload,
    Back,
    Forward,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ActInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    pub(crate) action: ActKind,
    /// Short-lived ref returned by the most recent snapshot or query.
    #[serde(rename = "ref")]
    pub(crate) reference: Option<String>,
    /// CSS selector alternative to `ref`.
    pub(crate) selector: Option<String>,
    /// Replacement text for `fill`. Never written to Horizon's audit log.
    pub(crate) value: Option<String>,
    /// Horizontal CSS-pixel delta for `scroll` (default 0).
    pub(crate) delta_x: Option<f64>,
    /// Vertical CSS-pixel delta for `scroll` (default 0).
    pub(crate) delta_y: Option<f64>,
    /// Consecutive trusted clicks for `click` (1-3, default 1).
    pub(crate) count: Option<u32>,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

impl ActInput {
    pub(crate) fn build_action(&self) -> Result<horizon_browser::BrowserControlAction, String> {
        use horizon_browser::BrowserControlAction;

        if !matches!(self.action, ActKind::Click) && self.count.is_some() {
            return Err("count is only accepted for click".to_string());
        }
        match self.action {
            ActKind::Click => {
                let count = self.count.unwrap_or(horizon_browser::DEFAULT_CLICK_COUNT);
                if !(horizon_browser::DEFAULT_CLICK_COUNT..=horizon_browser::MAX_CLICK_COUNT).contains(&count) {
                    return Err("click count must be between one and three".to_string());
                }
                Ok(BrowserControlAction::Click {
                    target: required_target(self.reference.as_deref(), self.selector.as_deref())?,
                    count,
                })
            }
            ActKind::Fill => Ok(BrowserControlAction::Fill {
                target: required_target(self.reference.as_deref(), self.selector.as_deref())?,
                value: self.value.clone().ok_or_else(|| "fill requires value".to_string())?,
            }),
            ActKind::Scroll => Ok(BrowserControlAction::Scroll {
                target: optional_target(self.reference.as_deref(), self.selector.as_deref())?,
                delta_x: self.delta_x.unwrap_or(0.0),
                delta_y: self.delta_y.unwrap_or(0.0),
            }),
            ActKind::Reload => no_target_or_value(self, BrowserControlAction::Reload),
            ActKind::Back => no_target_or_value(self, BrowserControlAction::Back),
            ActKind::Forward => no_target_or_value(self, BrowserControlAction::Forward),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EvaluateInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// JavaScript expression evaluated in the top-level document.
    pub(crate) expression: String,
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WaitState {
    Present,
    Visible,
    Hidden,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WaitInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// CSS selector to observe.
    pub(crate) selector: String,
    /// Desired selector state.
    pub(crate) state: WaitState,
    /// Total wait timeout in milliseconds (1-60000, default 10000).
    pub(crate) timeout_millis: Option<u64>,
    /// Delay between audited queries in milliseconds (100-2000, default 250).
    pub(crate) poll_millis: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct HandoffInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Short user-facing explanation of why steering is needed.
    pub(crate) reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AuditInput {
    /// Stable panel id returned by `browser_list`.
    pub(crate) panel_id: String,
    /// Optional action id filter.
    pub(crate) action_id: Option<String>,
    /// Maximum newest entries to return (1-500, default 100).
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ActionOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) completed: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct BoundsOutput {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NodeOutput {
    #[serde(rename = "ref")]
    pub(crate) reference: String,
    pub(crate) role: String,
    pub(crate) name: String,
    pub(crate) text: String,
    pub(crate) visible: bool,
    pub(crate) enabled: bool,
    pub(crate) bounds: Option<BoundsOutput>,
}

impl From<BrowserNode> for NodeOutput {
    fn from(value: BrowserNode) -> Self {
        Self {
            reference: value.reference,
            role: value.role,
            name: value.name,
            text: value.text,
            visible: value.visible,
            enabled: value.enabled,
            bounds: value.bounds.map(BoundsOutput::from),
        }
    }
}

impl From<BrowserBounds> for BoundsOutput {
    fn from(value: BrowserBounds) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SnapshotOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) nodes: Vec<NodeOutput>,
}

impl SnapshotOutput {
    pub(crate) fn new(panel_id: String, action_id: String, snapshot: BrowserSnapshot) -> Self {
        Self {
            panel_id,
            action_id,
            generation: snapshot.generation,
            revision: snapshot.revision,
            url: snapshot.url,
            title: snapshot.title,
            nodes: snapshot.nodes.into_iter().map(NodeOutput::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct NodesOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) generation: u64,
    pub(crate) revision: u64,
    pub(crate) nodes: Vec<NodeOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct EvaluateOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) value: serde_json::Value,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct WaitOutput {
    pub(crate) panel_id: String,
    pub(crate) action_id: String,
    pub(crate) state: String,
    pub(crate) nodes: Vec<NodeOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct HandoffOutput {
    pub(crate) panel_id: String,
    pub(crate) request_id: String,
    pub(crate) handoff_pending: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct AuditOutput {
    pub(crate) panel_id: String,
    pub(crate) entries: Vec<serde_json::Value>,
}

fn backend_name(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::ChromiumCdp => "chromium",
        BackendKind::FirefoxBidi => "firefox",
        BackendKind::SafariWebDriver => "safari",
    }
}

fn semantic_capabilities(backend: BackendKind) -> Vec<String> {
    let mut capabilities = [
        "navigate", "snapshot", "query", "click", "fill", "scroll", "reload", "back", "forward", "evaluate", "wait",
        "handoff", "audit",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if backend != BackendKind::SafariWebDriver {
        capabilities.extend(
            [
                "network_capture",
                "http_response_body_capture",
                "websocket_capture",
                "ndjson_export",
            ]
            .map(str::to_string),
        );
    }
    capabilities
}

fn required_target(reference: Option<&str>, selector: Option<&str>) -> Result<BrowserTarget, String> {
    optional_target(reference, selector)?.ok_or_else(|| "action requires exactly one of ref or selector".to_string())
}

fn optional_target(reference: Option<&str>, selector: Option<&str>) -> Result<Option<BrowserTarget>, String> {
    match (reference, selector) {
        (Some(_), Some(_)) => Err("provide ref or selector, not both".to_string()),
        (Some(reference), None) => Ok(Some(BrowserTarget::Ref {
            reference: reference.to_string(),
        })),
        (None, Some(selector)) => Ok(Some(BrowserTarget::Selector {
            selector: selector.to_string(),
        })),
        (None, None) => Ok(None),
    }
}

fn no_target_or_value(
    input: &ActInput,
    action: horizon_browser::BrowserControlAction,
) -> Result<horizon_browser::BrowserControlAction, String> {
    if input.reference.is_some()
        || input.selector.is_some()
        || input.value.is_some()
        || input.delta_x.is_some()
        || input.delta_y.is_some()
        || input.count.is_some()
    {
        Err("navigation-history actions do not accept target, value, deltas, or count".to_string())
    } else {
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(action: ActKind) -> ActInput {
        ActInput {
            panel_id: "panel".to_string(),
            action,
            reference: None,
            selector: None,
            value: None,
            delta_x: None,
            delta_y: None,
            count: None,
            timeout_millis: None,
        }
    }

    #[test]
    fn semantic_actions_require_unambiguous_targets() {
        let mut click = act(ActKind::Click);
        assert!(click.build_action().is_err());
        click.reference = Some("g1s1e1".to_string());
        assert!(matches!(
            click.build_action(),
            Ok(horizon_browser::BrowserControlAction::Click {
                target: BrowserTarget::Ref { .. },
                count: horizon_browser::DEFAULT_CLICK_COUNT,
            })
        ));
        click.selector = Some("button".to_string());
        assert!(click.build_action().is_err());
    }

    #[test]
    fn fill_text_is_not_part_of_the_action_shape_error() {
        let mut fill = act(ActKind::Fill);
        fill.value = Some("private value".to_string());
        let Err(error) = fill.build_action() else {
            panic!("fill without a target must fail");
        };
        assert!(!error.contains("private value"));
    }

    #[test]
    fn history_actions_reject_ignored_fields() {
        let mut reload = act(ActKind::Reload);
        reload.delta_y = Some(1.0);
        assert!(reload.build_action().is_err());
    }

    #[test]
    fn click_count_is_bounded_and_click_only() {
        let mut click = act(ActKind::Click);
        click.selector = Some("#row".to_string());
        click.count = Some(2);
        assert!(matches!(
            click.build_action(),
            Ok(horizon_browser::BrowserControlAction::Click { count: 2, .. })
        ));

        click.count = Some(0);
        assert!(click.build_action().is_err());
        click.count = Some(4);
        assert!(click.build_action().is_err());

        let mut scroll = act(ActKind::Scroll);
        scroll.count = Some(2);
        assert!(scroll.build_action().is_err());
    }
}
