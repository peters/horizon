//! Privacy-aware action audit values for embedders.
//!
//! The engine emits typed, redacted records through
//! [`BrowserCoordination`](crate::BrowserCoordination).
//! It never chooses a retention or storage policy. Horizon appends the values
//! to a private JSONL journal; another application can use a database, an
//! in-memory observer, or no audit sink at all.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::control::BrowserControlAction;
use crate::session::BrowserCommand;
use crate::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};

static AUDIT_ID_COUNTER: AtomicU64 = AtomicU64::new(0);
const POINTER_AUDIT_INTERVAL: Duration = Duration::from_millis(100);

/// Bounds durable audit work for high-rate, already-coalesced pointer motion.
/// Presses, releases, wheel input, keys, and page actions are always recorded.
#[derive(Debug, Default)]
pub(crate) struct BrowserAuditSampler {
    last_pointer_move: Option<Instant>,
}

impl BrowserAuditSampler {
    pub(crate) fn should_record(&mut self, command: &BrowserCommand) -> bool {
        self.should_record_at(command, Instant::now())
    }

    fn should_record_at(&mut self, command: &BrowserCommand, now: Instant) -> bool {
        if !matches!(command, BrowserCommand::Input(BrowserInput::MouseMove { .. })) {
            return true;
        }
        if self
            .last_pointer_move
            .is_some_and(|last| now.saturating_duration_since(last) < POINTER_AUDIT_INTERVAL)
        {
            return false;
        }
        self.last_pointer_move = Some(now);
        true
    }
}

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
    /// The backend handler completed the requested control operation.
    Completed,
    /// Refused before backend dispatch.
    Rejected,
    /// The backend accepted the request but could not complete it.
    Failed,
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
    Snapshot {
        max_nodes: u32,
    },
    Query {
        selector_characters: usize,
        max_results: u32,
    },
    Click {
        target: String,
        #[serde(default = "default_click_count")]
        count: u32,
    },
    Fill {
        target: String,
        characters: usize,
    },
    Scroll {
        target: Option<String>,
        delta_x: f64,
        delta_y: f64,
    },
    Evaluate {
        expression_characters: usize,
    },
    Network {
        operation: crate::BrowserNetworkOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_http: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_http_bodies: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_websocket: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_sent: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_received: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url_filter_count: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_payload_bytes: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_file_bytes: Option<u64>,
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
            BrowserControlAction::Snapshot { max_nodes } => Self::Snapshot { max_nodes: *max_nodes },
            BrowserControlAction::Query { selector, max_results } => Self::Query {
                selector_characters: selector.chars().count(),
                max_results: *max_results,
            },
            BrowserControlAction::Click { target, count } => Self::Click {
                target: audit_target(target),
                count: *count,
            },
            BrowserControlAction::Fill { target, value } => Self::Fill {
                target: audit_target(target),
                characters: value.chars().count(),
            },
            BrowserControlAction::Scroll {
                target,
                delta_x,
                delta_y,
            } => Self::Scroll {
                target: target.as_ref().map(audit_target),
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
            BrowserControlAction::Evaluate { expression } => Self::Evaluate {
                expression_characters: expression.chars().count(),
            },
            BrowserControlAction::Network { operation, options } => {
                let options =
                    (*operation == crate::BrowserNetworkOperation::Start).then(|| options.clone().unwrap_or_default());
                Self::Network {
                    operation: *operation,
                    include_http: options.as_ref().map(|options| options.include_http),
                    include_http_bodies: options.as_ref().map(|options| options.include_http_bodies),
                    include_websocket: options.as_ref().map(|options| options.include_websocket),
                    include_sent: options.as_ref().map(|options| options.frames.include_sent),
                    include_received: options.as_ref().map(|options| options.frames.include_received),
                    url_filter_count: options.as_ref().map(|options| options.url_patterns.len()),
                    max_payload_bytes: options.as_ref().map(|options| options.max_payload_bytes),
                    max_file_bytes: options.as_ref().map(|options| options.max_file_bytes),
                }
            }
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

fn audit_target(target: &crate::BrowserTarget) -> String {
    match target {
        crate::BrowserTarget::Ref { reference } => reference.clone(),
        crate::BrowserTarget::Selector { selector } => format!("selector:{}", selector.chars().count()),
    }
}

const fn default_click_count() -> u32 {
    crate::DEFAULT_CLICK_COUNT
}

pub(crate) fn redact_url(url: &str) -> String {
    let trimmed = url.trim();
    let scheme_end = trimmed.find(':');
    if scheme_end.is_some_and(|end| {
        !matches!(
            &trimmed[..end].to_ascii_lowercase()[..],
            "http" | "https" | "ws" | "wss" | "file" | "about"
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
    fn websocket_urls_keep_the_endpoint_but_redact_credentials_and_query_data() {
        assert_eq!(
            redact_url("wss://user:secret@example.test/market?token=private#fragment"),
            "wss://<redacted>@example.test/market?<redacted>#<redacted>"
        );
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

    #[test]
    fn semantic_audits_redact_selectors_scripts_and_fill_values() {
        let query = BrowserAuditAction::from_control(&BrowserControlAction::Query {
            selector: "input[value='secret']".to_string(),
            max_results: 2,
        });
        let fill = BrowserAuditAction::from_control(&BrowserControlAction::Fill {
            target: crate::BrowserTarget::Selector {
                selector: "#password".to_string(),
            },
            value: "correct horse battery staple".to_string(),
        });
        let evaluate = BrowserAuditAction::from_control(&BrowserControlAction::Evaluate {
            expression: "document.cookie".to_string(),
        });
        let json = serde_json::to_string(&[query, fill, evaluate]).unwrap_or_default();

        assert!(!json.contains("secret"));
        assert!(!json.contains("password"));
        assert!(!json.contains("correct horse"));
        assert!(!json.contains("document.cookie"));
    }

    #[test]
    fn semantic_click_audit_records_the_click_count() {
        let click = BrowserAuditAction::from_control(&BrowserControlAction::Click {
            target: crate::BrowserTarget::Selector {
                selector: "#private-row".to_string(),
            },
            count: 2,
        });

        assert_eq!(
            click,
            BrowserAuditAction::Click {
                target: "selector:12".to_string(),
                count: 2,
            }
        );
    }

    #[test]
    fn high_rate_pointer_motion_is_sampled_but_state_changes_are_not() {
        let started = Instant::now();
        let movement = BrowserCommand::Input(BrowserInput::MouseMove {
            x: 10.0,
            y: 20.0,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        });
        let release = BrowserCommand::Input(BrowserInput::MouseRelease {
            x: 10.0,
            y: 20.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        });
        let mut sampler = BrowserAuditSampler::default();

        assert!(sampler.should_record_at(&movement, started));
        assert!(!sampler.should_record_at(&movement, started + Duration::from_millis(99)));
        assert!(sampler.should_record_at(&release, started + Duration::from_millis(99)));
        assert!(sampler.should_record_at(&movement, started + POINTER_AUDIT_INTERVAL));
    }
}
