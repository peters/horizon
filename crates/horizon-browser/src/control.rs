//! Backend-neutral external control requests.
//!
//! A host can expose these values through any authenticated IPC it owns.
//! Horizon uses its locked local manifest queue; the browser engine itself
//! stays independent of that filesystem and of Horizon's panel model.

use crate::session::BrowserCommand;
use crate::{BrowserInput, BrowserKey, BrowserTarget};

const MAX_NAVIGATION_BYTES: usize = 8 * 1024;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_SELECTOR_BYTES: usize = 16 * 1024;
const MAX_EXPRESSION_BYTES: usize = 128 * 1024;
pub const MAX_SNAPSHOT_NODES: u32 = 1_000;
pub const MAX_QUERY_RESULTS: u32 = 250;

/// One action an external controller can ask a live browser session to take.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserControlAction {
    Navigate {
        url: String,
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
}

impl BrowserControlAction {
    /// Validate untrusted host/agent input before it enters a driver queue.
    ///
    /// # Errors
    /// Returns a stable explanation when a URL, text payload, button mask,
    /// click count, or numeric coordinate is outside the engine contract.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Navigate { url } => validate_navigation(url),
            Self::Input { input } => validate_input(input),
            Self::Snapshot { max_nodes } => validate_snapshot_limit(*max_nodes),
            Self::Query { selector, max_results } => {
                validate_selector(selector)?;
                validate_query_limit(*max_results)
            }
            Self::Click { target } => validate_target(target),
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
            Self::Reload | Self::Back | Self::Forward => Ok(()),
        }
    }

    #[must_use]
    pub fn to_command(&self) -> Option<BrowserCommand> {
        match self {
            Self::Navigate { url } => Some(BrowserCommand::Navigate(url.clone())),
            Self::Reload => Some(BrowserCommand::Reload),
            Self::Back => Some(BrowserCommand::Back),
            Self::Forward => Some(BrowserCommand::Forward),
            Self::Input { input } => Some(BrowserCommand::Input(input.clone())),
            Self::Snapshot { .. }
            | Self::Query { .. }
            | Self::Click { .. }
            | Self::Fill { .. }
            | Self::Scroll { .. }
            | Self::Evaluate { .. } => None,
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
            if !(1..=3).contains(click_count) {
                return Err("click count must be between one and three");
            }
            Ok(())
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
        };

        assert!(invalid_point.validate().is_err());
        assert!(oversized.validate().is_err());
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
    }
}
