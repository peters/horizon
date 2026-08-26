//! Backend-neutral external control requests.
//!
//! A host can expose these values through any authenticated IPC it owns.
//! Horizon uses its locked local manifest queue; the browser engine itself
//! stays independent of that filesystem and of Horizon's panel model.

use crate::session::BrowserCommand;
use crate::{BrowserInput, BrowserKey};

const MAX_NAVIGATION_BYTES: usize = 8 * 1024;
const MAX_TEXT_BYTES: usize = 64 * 1024;

/// One action an external controller can ask a live browser session to take.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserControlAction {
    Navigate { url: String },
    Reload,
    Back,
    Forward,
    Input { input: BrowserInput },
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
            Self::Reload | Self::Back | Self::Forward => Ok(()),
        }
    }

    #[must_use]
    pub fn into_command(self) -> BrowserCommand {
        match self {
            Self::Navigate { url } => BrowserCommand::Navigate(url),
            Self::Reload => BrowserCommand::Reload,
            Self::Back => BrowserCommand::Back,
            Self::Forward => BrowserCommand::Forward,
            Self::Input { input } => BrowserCommand::Input(input),
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
}
