//! Privacy-aware action audit values for embedders.
//!
//! The engine emits typed, redacted records through
//! [`BrowserCoordination`](crate::BrowserCoordination).
//! It never chooses a retention or storage policy. Horizon appends the values
//! to a private JSONL journal; another application can use a database, an
//! in-memory observer, or no audit sink at all.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::control::BrowserControlAction;
use crate::session::BrowserCommand;
use crate::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};

static AUDIT_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserAuditActor {
    User,
    Agent { name: String },
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAuditStatus {
    /// Accepted by the host's bounded queue but not yet claimed by a driver.
    Queued,
    /// Submitted to the backend adapter. This is not page-level success proof.
    Dispatched,
    /// Refused before backend dispatch.
    Rejected,
}

/// Redacted action shape. Text content and URL query/fragment values are
/// deliberately excluded so auditing never becomes a credential logger.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserAuditAction {
    Navigate {
        destination: String,
    },
    Reload,
    Back,
    Forward,
    Viewport {
        width: u32,
        height: u32,
    },
    PointerMove {
        x: f64,
        y: f64,
        buttons: u32,
    },
    PointerPress {
        x: f64,
        y: f64,
        button: BrowserButton,
        click_count: u32,
        buttons: u32,
    },
    PointerRelease {
        x: f64,
        y: f64,
        button: BrowserButton,
        click_count: u32,
        buttons: u32,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    KeyDown {
        key: String,
        text_characters: usize,
        modifiers: BrowserModifiers,
        repeat: bool,
        edit_command: Option<BrowserEditCommand>,
    },
    KeyUp {
        key: String,
        text_characters: usize,
        modifiers: BrowserModifiers,
    },
    InsertText {
        characters: usize,
    },
    HandoffRequested,
    HandoffDone,
    Stop,
}

impl BrowserAuditAction {
    #[must_use]
    pub fn from_control(action: &BrowserControlAction) -> Self {
        match action {
            BrowserControlAction::Navigate { url } => Self::Navigate {
                destination: redact_url(url),
            },
            BrowserControlAction::Reload => Self::Reload,
            BrowserControlAction::Back => Self::Back,
            BrowserControlAction::Forward => Self::Forward,
            BrowserControlAction::Input { input } => Self::from_input(input),
        }
    }

    #[must_use]
    pub fn from_command(command: &BrowserCommand) -> Self {
        match command {
            BrowserCommand::Navigate(url) => Self::Navigate {
                destination: redact_url(url),
            },
            BrowserCommand::Reload => Self::Reload,
            BrowserCommand::Back => Self::Back,
            BrowserCommand::Forward => Self::Forward,
            BrowserCommand::SetViewport { width, height } => Self::Viewport {
                width: *width,
                height: *height,
            },
            BrowserCommand::Input(input) => Self::from_input(input),
            BrowserCommand::HandoffDone => Self::HandoffDone,
            BrowserCommand::Stop => Self::Stop,
        }
    }

    fn from_input(input: &BrowserInput) -> Self {
        match input {
            BrowserInput::MouseMove { x, y, buttons, .. } => Self::PointerMove {
                x: *x,
                y: *y,
                buttons: *buttons,
            },
            BrowserInput::MousePress {
                x,
                y,
                button,
                click_count,
                buttons,
                ..
            } => Self::PointerPress {
                x: *x,
                y: *y,
                button: *button,
                click_count: *click_count,
                buttons: *buttons,
            },
            BrowserInput::MouseRelease {
                x,
                y,
                button,
                click_count,
                buttons,
                ..
            } => Self::PointerRelease {
                x: *x,
                y: *y,
                button: *button,
                click_count: *click_count,
                buttons: *buttons,
            },
            BrowserInput::Wheel {
                x, y, delta_x, delta_y, ..
            } => Self::Wheel {
                x: *x,
                y: *y,
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
            BrowserInput::KeyDown {
                key,
                text,
                modifiers,
                repeat,
                edit_command,
                ..
            } => Self::KeyDown {
                key: audit_key(*key),
                text_characters: text.as_deref().map_or(0, |text| text.chars().count()),
                modifiers: *modifiers,
                repeat: *repeat,
                edit_command: *edit_command,
            },
            BrowserInput::KeyUp {
                key, text, modifiers, ..
            } => Self::KeyUp {
                key: audit_key(*key),
                text_characters: text.as_deref().map_or(0, |text| text.chars().count()),
                modifiers: *modifiers,
            },
            BrowserInput::InsertText { text } => Self::InsertText {
                characters: text.chars().count(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct BrowserAuditEntry {
    pub schema_version: u32,
    pub event_id: String,
    pub action_id: String,
    pub at_millis: i64,
    pub actor: BrowserAuditActor,
    pub status: BrowserAuditStatus,
    pub action: BrowserAuditAction,
}

impl BrowserAuditEntry {
    #[must_use]
    pub fn new(
        action_id: String,
        actor: BrowserAuditActor,
        status: BrowserAuditStatus,
        action: BrowserAuditAction,
    ) -> Self {
        Self {
            schema_version: 1,
            event_id: new_id("event"),
            action_id,
            at_millis: now_millis(),
            actor,
            status,
            action,
        }
    }
}

#[must_use]
pub fn new_action_id() -> String {
    new_id("action")
}

fn new_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = AUDIT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{nanos:x}-{sequence:x}", std::process::id())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}

fn audit_key(key: BrowserKey) -> String {
    match key {
        BrowserKey::Char(_) => "printable".to_string(),
        other => format!("{other:?}"),
    }
}

fn redact_url(url: &str) -> String {
    let trimmed = url.trim();
    let scheme_end = trimmed.find(':');
    if scheme_end.is_some_and(|end| {
        !matches!(
            &trimmed[..end].to_ascii_lowercase()[..],
            "http" | "https" | "file" | "about"
        )
    }) {
        return scheme_end.map_or_else(
            || "<redacted>".to_string(),
            |end| format!("{}:<redacted>", &trimmed[..end]),
        );
    }

    let query = trimmed.find('?');
    let fragment = trimmed.find('#');
    let suffix_at = query.into_iter().chain(fragment).min().unwrap_or(trimmed.len());
    let mut base = trimmed[..suffix_at].to_string();
    if let Some(authority_start) = base.find("://").map(|index| index + 3) {
        let authority_end = base[authority_start..]
            .find('/')
            .map_or(base.len(), |index| authority_start + index);
        if let Some(userinfo_end) = base[authority_start..authority_end].rfind('@') {
            let userinfo_end = authority_start + userinfo_end;
            base.replace_range(authority_start..userinfo_end, "<redacted>");
        }
    }
    if query.is_some() {
        base.push_str("?<redacted>");
    }
    if fragment.is_some() {
        base.push_str("#<redacted>");
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audits_redact_navigation_secrets_and_text_content() {
        let navigation = BrowserAuditAction::from_control(&BrowserControlAction::Navigate {
            url: "https://user:secret@example.test/path?token=secret#private".to_string(),
        });
        let text = BrowserAuditAction::from_control(&BrowserControlAction::Input {
            input: BrowserInput::InsertText {
                text: "correct horse battery staple".to_string(),
            },
        });

        assert_eq!(
            navigation,
            BrowserAuditAction::Navigate {
                destination: "https://<redacted>@example.test/path?<redacted>#<redacted>".to_string()
            }
        );
        assert_eq!(text, BrowserAuditAction::InsertText { characters: 28 });
    }

    #[test]
    fn audits_never_store_printable_key_values() {
        let action = BrowserAuditAction::from_control(&BrowserControlAction::Input {
            input: BrowserInput::KeyDown {
                physical_key: Some(BrowserKey::Char('p')),
                key: BrowserKey::Char('s'),
                text: Some("s".to_string()),
                modifiers: BrowserModifiers::none(),
                repeat: false,
                edit_command: None,
            },
        });
        let json = serde_json::to_string(&action).unwrap_or_default();

        assert!(json.contains("printable"));
        assert!(!json.contains("\"s\""));
    }
}
