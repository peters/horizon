//! Backend-neutral external control requests.
//!
//! A host can expose these values through any authenticated IPC it owns.
//! Horizon uses its locked local manifest queue; the browser engine itself
//! stays independent of that filesystem and of Horizon's panel model.

use crate::{
    BrowserCommand, BrowserInput, BrowserKey, BrowserNetworkCaptureOptions, BrowserNetworkOperation, BrowserTarget,
};

const MAX_NAVIGATION_BYTES: usize = 8 * 1024;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_SELECTOR_BYTES: usize = 16 * 1024;
const MAX_EXPRESSION_BYTES: usize = 128 * 1024;
pub const MAX_SNAPSHOT_NODES: u32 = 1_000;
pub const MAX_QUERY_RESULTS: u32 = 250;
pub const DEFAULT_CLICK_COUNT: u32 = 1;
pub const MAX_CLICK_COUNT: u32 = 3;
/// Longest bounded wait an engine performs for one navigation action.
pub const MAX_NAVIGATION_TIMEOUT_MILLIS: u64 = 60_000;
/// Wait applied when a navigation action does not carry its own bound.
pub const DEFAULT_NAVIGATION_TIMEOUT_MILLIS: u64 = 15_000;

/// How far a navigation action waits before it reports its outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationWait {
    /// Return as soon as the engine dispatched the navigation command to the
    /// backend; nothing about the destination is known yet.
    Dispatched,
    /// Return once the top-level document committed (URL is authoritative).
    #[default]
    Commit,
    /// Return once the committed document fired `DOMContentLoaded`.
    #[serde(alias = "domcontentloaded")]
    DomContentLoaded,
}

/// Resolve an omnibox-style navigation target without overriding an explicit
/// scheme. Hostnames default to HTTPS; callers can still request HTTP.
#[must_use]
pub fn normalize_navigation_target(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() || has_explicit_browser_scheme(input) {
        return input.to_string();
    }
    if input.starts_with("//") {
        format!("https:{input}")
    } else {
        format!("https://{input}")
    }
}

fn has_explicit_browser_scheme(input: &str) -> bool {
    input.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme.chars().enumerate().all(|(index, character)| {
                character.is_ascii_alphabetic()
                    || index > 0 && (character.is_ascii_digit() || "+-.".contains(character))
            })
    }) || [
        "about:",
        "blob:",
        "data:",
        "file:",
        "ftp:",
        "javascript:",
        "mailto:",
        "view-source:",
    ]
    .iter()
    .any(|scheme| {
        input
            .get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    })
}

/// One action an external controller can ask a live browser session to take.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserControlAction {
    Navigate {
        url: String,
        /// Readiness the engine waits for before reporting the outcome.
        #[serde(default)]
        wait: NavigationWait,
        /// Bound on that wait in milliseconds; the engine reports a typed
        /// `timed_out` outcome with the latest page state when it elapses.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_millis: Option<u64>,
    },
    Reload,
    Back,
    Forward,
    Input {
        input: BrowserInput,
    },
    /// Return a bounded semantic DOM snapshot with short-lived element refs.
    Snapshot {
        max_nodes: u32,
    },
    /// Find elements with a CSS selector and assign refs to this result set.
    Query {
        selector: String,
        max_results: u32,
    },
    /// Click the center of a visible element through the backend input path.
    Click {
        target: BrowserTarget,
        /// Number of consecutive clicks dispatched as one semantic action.
        #[serde(default = "default_click_count")]
        count: u32,
    },
    /// Focus, clear, and type into an editable element through backend input.
    Fill {
        target: BrowserTarget,
        value: String,
    },
    /// Scroll the top-level viewport or a selected element.
    Scroll {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<BrowserTarget>,
        delta_x: f64,
        delta_y: f64,
    },
    /// Evaluate JavaScript in the current top-level document and return JSON.
    Evaluate {
        expression: String,
    },
    /// Start, inspect, or stop a bounded network capture export.
    Network {
        operation: BrowserNetworkOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<BrowserNetworkCaptureOptions>,
    },
}

impl BrowserControlAction {
    /// Validate untrusted host/agent input before it enters a driver queue.
    ///
    /// # Errors
    /// Returns a stable explanation when a URL, text payload, button mask,
    /// click count, or numeric coordinate is outside the engine contract.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Navigate {
                url, timeout_millis, ..
            } => {
                validate_navigation(url)?;
                validate_navigation_timeout(*timeout_millis)
            }
            Self::Input { input } => validate_input(input),
            Self::Snapshot { max_nodes } => validate_snapshot_limit(*max_nodes),
            Self::Query { selector, max_results } => {
                validate_selector(selector)?;
                validate_query_limit(*max_results)
            }
            Self::Click { target, count } => {
                validate_target(target)?;
                validate_click_count(*count)
            }
            Self::Fill { target, value } => {
                validate_target(target)?;
                validate_text(value)
            }
            Self::Scroll {
                target,
                delta_x,
                delta_y,
            } => {
                if let Some(target) = target {
                    validate_target(target)?;
                }
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err("scroll delta must be finite");
                }
                Ok(())
            }
            Self::Evaluate { expression } => validate_expression(expression),
            Self::Network { operation, options } => match operation {
                BrowserNetworkOperation::Start => options.clone().unwrap_or_default().validate(),
                BrowserNetworkOperation::Status | BrowserNetworkOperation::Stop if options.is_some() => {
                    Err("network status and stop do not accept capture options")
                }
                BrowserNetworkOperation::Status | BrowserNetworkOperation::Stop => Ok(()),
            },
            Self::Reload | Self::Back | Self::Forward => Ok(()),
        }
    }

    #[must_use]
    pub fn to_command(&self) -> Option<BrowserCommand> {
        match self {
            Self::Navigate { url, .. } => Some(BrowserCommand::Navigate(normalize_navigation_target(url))),
            Self::Reload => Some(BrowserCommand::Reload),
            Self::Back => Some(BrowserCommand::Back),
            Self::Forward => Some(BrowserCommand::Forward),
            Self::Input { input } => Some(BrowserCommand::Input(input.clone())),
            Self::Snapshot { .. }
            | Self::Query { .. }
            | Self::Click { .. }
            | Self::Fill { .. }
            | Self::Scroll { .. }
            | Self::Evaluate { .. }
            | Self::Network { .. } => None,
        }
    }
}

/// Host-issued identity and actor metadata for one external action.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct AgentAction {
    pub action_id: String,
    pub actor: String,
    pub requested_at_millis: i64,
    pub action: BrowserControlAction,
}

fn validate_navigation(url: &str) -> Result<(), &'static str> {
    if url.trim().is_empty() {
        return Err("navigation URL must not be empty");
    }
    if url.len() > MAX_NAVIGATION_BYTES {
        return Err("navigation URL is too long");
    }
    if url.chars().any(char::is_control) {
        return Err("navigation URL contains control characters");
    }
    Ok(())
}

fn validate_navigation_timeout(timeout_millis: Option<u64>) -> Result<(), &'static str> {
    match timeout_millis {
        Some(0) => Err("navigation timeout must be at least 1 ms"),
        Some(value) if value > MAX_NAVIGATION_TIMEOUT_MILLIS => Err("navigation timeout exceeds the engine bound"),
        _ => Ok(()),
    }
}

fn validate_snapshot_limit(value: u32) -> Result<(), &'static str> {
    if value == 0 {
        return Err("snapshot node limit must be at least one");
    }
    if value > MAX_SNAPSHOT_NODES {
        return Err("snapshot node limit is too large");
    }
    Ok(())
}

fn validate_query_limit(value: u32) -> Result<(), &'static str> {
    if value == 0 {
        return Err("query result limit must be at least one");
    }
    if value > MAX_QUERY_RESULTS {
        return Err("query result limit is too large");
    }
    Ok(())
}

fn validate_target(target: &BrowserTarget) -> Result<(), &'static str> {
    match target {
        BrowserTarget::Ref { reference } => {
            if reference.trim().is_empty() || reference.len() > 128 || reference.chars().any(char::is_control) {
                Err("element reference must be a short printable value")
            } else {
                Ok(())
            }
        }
        BrowserTarget::Selector { selector } => validate_selector(selector),
    }
}

fn validate_selector(selector: &str) -> Result<(), &'static str> {
    if selector.trim().is_empty() {
        return Err("selector must not be empty");
    }
    if selector.len() > MAX_SELECTOR_BYTES {
        return Err("selector is too long");
    }
    if selector.chars().any(char::is_control) {
        return Err("selector contains control characters");
    }
    Ok(())
}

fn validate_expression(expression: &str) -> Result<(), &'static str> {
    if expression.trim().is_empty() {
        return Err("JavaScript expression must not be empty");
    }
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err("JavaScript expression is too long");
    }
    Ok(())
}

fn validate_input(input: &BrowserInput) -> Result<(), &'static str> {
    match input {
        BrowserInput::MouseMove { x, y, buttons, .. } => {
            validate_point(*x, *y)?;
            validate_buttons(*buttons)
        }
        BrowserInput::MousePress {
            x,
            y,
            click_count,
            buttons,
            ..
        }
        | BrowserInput::MouseRelease {
            x,
            y,
            click_count,
            buttons,
            ..
        } => {
            validate_point(*x, *y)?;
            validate_buttons(*buttons)?;
            validate_click_count(*click_count)
        }
        BrowserInput::Wheel {
            x, y, delta_x, delta_y, ..
        } => {
            validate_point(*x, *y)?;
            if !delta_x.is_finite() || !delta_y.is_finite() {
                return Err("wheel delta must be finite");
            }
            Ok(())
        }
        BrowserInput::KeyDown {
            physical_key,
            key,
            text,
            ..
        }
        | BrowserInput::KeyUp {
            physical_key,
            key,
            text,
            ..
        } => {
            validate_key(*key)?;
            if let Some(physical_key) = physical_key {
                validate_key(*physical_key)?;
            }
            validate_optional_text(text.as_deref())
        }
        BrowserInput::InsertText { text } => validate_text(text),
    }
}

const fn default_click_count() -> u32 {
    DEFAULT_CLICK_COUNT
}

fn validate_click_count(click_count: u32) -> Result<(), &'static str> {
    if (DEFAULT_CLICK_COUNT..=MAX_CLICK_COUNT).contains(&click_count) {
        Ok(())
    } else {
        Err("click count must be between one and three")
    }
}

fn validate_point(x: f64, y: f64) -> Result<(), &'static str> {
    if !x.is_finite() || !y.is_finite() {
        return Err("pointer coordinates must be finite");
    }
    Ok(())
}

fn validate_buttons(buttons: u32) -> Result<(), &'static str> {
    if buttons & !0b111 == 0 {
        Ok(())
    } else {
        Err("pointer button mask contains unsupported bits")
    }
}

fn validate_key(key: BrowserKey) -> Result<(), &'static str> {
    if matches!(key, BrowserKey::Char(character) if character.is_control()) {
        Err("printable key contains a control character")
    } else {
        Ok(())
    }
}

fn validate_optional_text(text: Option<&str>) -> Result<(), &'static str> {
    text.map_or(Ok(()), validate_text)
}

fn validate_text(text: &str) -> Result<(), &'static str> {
    if text.len() > MAX_TEXT_BYTES {
        Err("input text is too long")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserButton, BrowserModifiers};

    #[test]
    fn external_controls_roundtrip_without_horizon_types() {
        let action = BrowserControlAction::Input {
            input: BrowserInput::MousePress {
                x: 12.0,
                y: 18.0,
                button: BrowserButton::Left,
                click_count: 1,
                buttons: 1,
                modifiers: BrowserModifiers::none(),
            },
        };
        let encoded = serde_json::to_string(&action).unwrap_or_default();
        let decoded = serde_json::from_str::<BrowserControlAction>(&encoded).unwrap_or(BrowserControlAction::Reload);

        assert_eq!(decoded, action);
        assert!(decoded.validate().is_ok());
    }

    #[test]
    fn external_controls_reject_nonfinite_or_oversized_payloads() {
        let invalid_point = BrowserControlAction::Input {
            input: BrowserInput::MouseMove {
                x: f64::NAN,
                y: 0.0,
                buttons: 0,
                modifiers: BrowserModifiers::none(),
            },
        };
        let oversized = BrowserControlAction::Navigate {
            url: "x".repeat(MAX_NAVIGATION_BYTES + 1),
            wait: NavigationWait::default(),
            timeout_millis: None,
        };

        assert!(invalid_point.validate().is_err());
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn navigate_actions_default_to_a_committed_wait_and_keep_the_legacy_shape() {
        let legacy: BrowserControlAction =
            serde_json::from_value(serde_json::json!({ "type": "navigate", "url": "example.test" }))
                .expect("legacy navigate decodes");
        assert_eq!(
            legacy,
            BrowserControlAction::Navigate {
                url: "example.test".to_string(),
                wait: NavigationWait::Commit,
                timeout_millis: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&legacy).expect("encode"),
            serde_json::json!({ "type": "navigate", "url": "example.test", "wait": "commit" })
        );
        let explicit: BrowserControlAction = serde_json::from_value(serde_json::json!({
            "type": "navigate", "url": "example.test", "wait": "domcontentloaded", "timeout_millis": 2000
        }))
        .expect("aliased wait decodes");
        assert!(matches!(
            explicit,
            BrowserControlAction::Navigate {
                wait: NavigationWait::DomContentLoaded,
                timeout_millis: Some(2000),
                ..
            }
        ));
        assert!(explicit.validate().is_ok());
        assert_eq!(
            BrowserControlAction::Navigate {
                url: "example.test".to_string(),
                wait: NavigationWait::Commit,
                timeout_millis: Some(0),
            }
            .validate(),
            Err("navigation timeout must be at least 1 ms")
        );
        assert_eq!(
            BrowserControlAction::Navigate {
                url: "example.test".to_string(),
                wait: NavigationWait::Commit,
                timeout_millis: Some(MAX_NAVIGATION_TIMEOUT_MILLIS + 1),
            }
            .validate(),
            Err("navigation timeout exceeds the engine bound")
        );
    }

    #[test]
    fn navigation_defaults_to_https_without_overriding_explicit_schemes() {
        assert_eq!(normalize_navigation_target("e24.no/bors"), "https://e24.no/bors");
        assert_eq!(normalize_navigation_target("//e24.no/bors"), "https://e24.no/bors");
        assert_eq!(
            normalize_navigation_target("http://127.0.0.1:3000"),
            "http://127.0.0.1:3000"
        );
        assert_eq!(normalize_navigation_target("about:blank"), "about:blank");
        assert_eq!(
            normalize_navigation_target("file:/tmp/page.html"),
            "file:/tmp/page.html"
        );
        assert!(matches!(
            BrowserControlAction::Navigate {
                url: "example.test/path".to_string(),
                wait: NavigationWait::default(),
                timeout_millis: None,
            }
            .to_command(),
            Some(BrowserCommand::Navigate(url)) if url == "https://example.test/path"
        ));
    }

    #[test]
    fn semantic_controls_validate_limits_and_sensitive_payload_sizes() {
        assert!(
            BrowserControlAction::Snapshot {
                max_nodes: MAX_SNAPSHOT_NODES
            }
            .validate()
            .is_ok()
        );
        assert!(BrowserControlAction::Snapshot { max_nodes: 0 }.validate().is_err());
        assert!(
            BrowserControlAction::Query {
                selector: "button[data-action='save']".to_string(),
                max_results: 10,
            }
            .validate()
            .is_ok()
        );
        assert!(
            BrowserControlAction::Fill {
                target: BrowserTarget::Selector {
                    selector: "#password".to_string()
                },
                value: "x".repeat(MAX_TEXT_BYTES + 1),
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserControlAction::Click {
                target: BrowserTarget::Selector {
                    selector: "#row".to_string()
                },
                count: 2,
            }
            .validate()
            .is_ok()
        );
        assert!(
            BrowserControlAction::Click {
                target: BrowserTarget::Selector {
                    selector: "#row".to_string()
                },
                count: 0,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn click_deserialization_defaults_to_a_single_click() {
        let Ok(decoded) = serde_json::from_value::<BrowserControlAction>(serde_json::json!({
            "type": "click",
            "target": { "type": "selector", "selector": "#save" }
        })) else {
            panic!("click action should deserialize");
        };

        assert_eq!(
            decoded,
            BrowserControlAction::Click {
                target: BrowserTarget::Selector {
                    selector: "#save".to_string()
                },
                count: DEFAULT_CLICK_COUNT,
            }
        );
    }
}
