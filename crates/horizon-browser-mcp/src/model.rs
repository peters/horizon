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
    pub(crate) owner: Option<String>,
    pub(crate) owned_by_caller: bool,
    pub(crate) user_active: bool,
    pub(crate) handoff_pending: bool,
    pub(crate) capabilities: Vec<String>,
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
            owner,
            owned_by_caller,
            user_active,
            handoff_pending,
            capabilities: semantic_capabilities(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct BrowserListOutput {
    pub(crate) panels: Vec<BrowserPanel>,
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
    /// Per-action timeout in milliseconds (1-60000).
    pub(crate) timeout_millis: Option<u64>,
}

impl ActInput {
    pub(crate) fn build_action(&self) -> Result<horizon_browser::BrowserControlAction, String> {
        use horizon_browser::BrowserControlAction;

        match self.action {
            ActKind::Click => Ok(BrowserControlAction::Click {
                target: required_target(self.reference.as_deref(), self.selector.as_deref())?,
            }),
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

fn semantic_capabilities() -> Vec<String> {
    [
        "navigate", "snapshot", "query", "click", "fill", "scroll", "reload", "back", "forward", "evaluate", "wait",
        "handoff", "audit",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
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
    {
        Err("navigation-history actions do not accept target, value, or deltas".to_string())
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
                target: BrowserTarget::Ref { .. }
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
}
