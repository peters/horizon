use std::io;
use std::time::{Duration, Instant};

use horizon_browser::{
    AgentActionResult, BackendKind, BrowserActionOutcome, BrowserControlAction, BrowserControlValue,
};
use horizon_core::browser::manifest;
use thiserror::Error;

use crate::model::{BrowserPanel, ProtocolKind};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const RESULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
pub(crate) const DEFAULT_ACTION_TIMEOUT_MILLIS: u64 = 15_000;
pub(crate) const MAX_ACTION_TIMEOUT_MILLIS: u64 = 60_000;

#[derive(Clone, Debug)]
pub(crate) struct BrowserController {
    actor: String,
}

#[derive(Debug)]
pub(crate) struct ActionReceipt {
    pub(crate) action_id: String,
    pub(crate) value: BrowserControlValue,
}

#[derive(Debug, Error)]
pub(crate) enum ControlError {
    #[error("{operation}: {reason}")]
    Io {
        operation: &'static str,
        reason: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("browser action {action_id} timed out after {timeout_millis} ms; inspect browser_audit before retrying")]
    Timeout { action_id: String, timeout_millis: u64 },
    #[error("browser action {action_id} failed ({code}): {message}")]
    Browser {
        action_id: String,
        code: String,
        message: String,
    },
}

impl BrowserController {
    pub(crate) fn from_environment() -> Self {
        let fallback = format!("horizon-mcp:{}", std::process::id());
        let actor = std::env::var("HORIZON_BROWSER_ACTOR")
            .ok()
            .filter(|value| valid_actor(value))
            .unwrap_or(fallback);
        Self { actor }
    }

    #[cfg(test)]
    pub(crate) fn with_actor(actor: impl Into<String>) -> Self {
        Self { actor: actor.into() }
    }

    pub(crate) fn list_panels(&self) -> Vec<BrowserPanel> {
        let directory = manifest::default_manifest_dir();
        let mut panels = manifest::list_panels_in(&directory)
            .into_iter()
            .filter_map(|panel_id| manifest::read(&panel_id))
            .map(|manifest| BrowserPanel::from_manifest(manifest, &self.actor))
            .collect::<Vec<_>>();
        panels.sort_by(|left, right| left.panel_id.cmp(&right.panel_id));
        panels
    }

    pub(crate) async fn execute(
        &self,
        panel_id: &str,
        action: BrowserControlAction,
        timeout_millis: Option<u64>,
    ) -> Result<ActionReceipt, ControlError> {
        let timeout_millis = bounded_timeout(timeout_millis);
        self.ensure_claim(panel_id)?;
        let action_id = manifest::enqueue_action(panel_id, &self.actor, action)
            .map_err(|source| ControlError::internal_io("could not queue browser action", source))?;
        self.wait_for_result(panel_id, action_id, timeout_millis).await
    }

    pub(crate) fn request_handoff(&self, panel_id: &str, reason: &str) -> Result<String, ControlError> {
        self.ensure_claim(panel_id)?;
        manifest::request_handoff(panel_id, &self.actor, reason)
            .map_err(|source| ControlError::internal_io("could not request browser handoff", source))
    }

    pub(crate) fn read_audit(&self, panel_id: &str) -> Result<Vec<horizon_browser::BrowserAuditEntry>, ControlError> {
        tracing::debug!(actor = %self.actor, panel_id, "reading browser action audit");
        manifest::read_audit(panel_id)
            .map_err(|source| ControlError::internal_io("could not read browser audit", source))
    }

    fn ensure_claim(&self, panel_id: &str) -> Result<(), ControlError> {
        match manifest::heartbeat(panel_id, &self.actor) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                manifest::claim(panel_id, &self.actor, None)
                    .map_err(|source| ControlError::internal_io("could not claim browser panel", source))
            }
            Err(source) => Err(ControlError::internal_io("could not refresh browser ownership", source)),
        }
    }

    async fn wait_for_result(
        &self,
        panel_id: &str,
        action_id: String,
        timeout_millis: u64,
    ) -> Result<ActionReceipt, ControlError> {
        let started = Instant::now();
        let timeout = Duration::from_millis(timeout_millis);
        let mut last_heartbeat = started;
        loop {
            if let Some(result) = manifest::take_action_result(panel_id, &action_id)
                .map_err(|source| ControlError::internal_io("could not read browser action result", source))?
            {
                return outcome(result);
            }
            if started.elapsed() >= timeout {
                return Err(ControlError::Timeout {
                    action_id,
                    timeout_millis,
                });
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                manifest::heartbeat(panel_id, &self.actor).map_err(|source| {
                    ControlError::internal_io("lost browser ownership while waiting for a result", source)
                })?;
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(RESULT_POLL_INTERVAL).await;
        }
    }
}

impl ControlError {
    fn internal_io(operation: &'static str, source: io::Error) -> Self {
        tracing::warn!(operation, error = %source, "browser MCP host coordination failed");
        let reason = match source.kind() {
            io::ErrorKind::WouldBlock => "would block while the user is steering or the action queue is full",
            io::ErrorKind::PermissionDenied => "permission denied because browser panel ownership changed",
            io::ErrorKind::NotFound => "browser panel is not live",
            io::ErrorKind::InvalidInput => "invalid browser control input",
            io::ErrorKind::TimedOut => "host coordination timed out",
            _ => "internal host coordination error",
        };
        Self::Io {
            operation,
            reason,
            source,
        }
    }
}

fn bounded_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(DEFAULT_ACTION_TIMEOUT_MILLIS)
        .clamp(1, MAX_ACTION_TIMEOUT_MILLIS)
}

fn outcome(result: AgentActionResult) -> Result<ActionReceipt, ControlError> {
    match result.outcome {
        BrowserActionOutcome::Completed { value } => Ok(ActionReceipt {
            action_id: result.action_id,
            value,
        }),
        BrowserActionOutcome::Failed { error } => {
            let message = public_browser_error(&error.code, &error.message);
            if message != error.message.as_str() {
                tracing::warn!(code = %error.code, error = %error.message, "redacted private browser backend failure");
            }
            Err(ControlError::Browser {
                action_id: result.action_id,
                code: error.code,
                message,
            })
        }
    }
}

fn public_browser_error(code: &str, message: &str) -> String {
    if matches!(code, "protocol_error" | "input_failed")
        || ["ws://", "wss://", "http://127.0.0.1", "http://localhost"]
            .iter()
            .any(|marker| message.contains(marker))
    {
        "browser backend operation failed; inspect Horizon's local logs".to_string()
    } else {
        message.to_string()
    }
}

fn valid_actor(actor: &str) -> bool {
    !actor.trim().is_empty() && actor.len() <= 128 && !actor.chars().any(char::is_control)
}

pub(crate) fn protocol_kind(backend: BackendKind, websocket_negotiated: bool) -> ProtocolKind {
    match backend {
        BackendKind::ChromiumCdp => ProtocolKind::Cdp,
        BackendKind::FirefoxBidi | BackendKind::SafariWebDriver if websocket_negotiated => ProtocolKind::WebDriverBidi,
        BackendKind::FirefoxBidi | BackendKind::SafariWebDriver => ProtocolKind::WebDriver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_bounded() {
        assert_eq!(bounded_timeout(None), DEFAULT_ACTION_TIMEOUT_MILLIS);
        assert_eq!(bounded_timeout(Some(0)), 1);
        assert_eq!(bounded_timeout(Some(u64::MAX)), MAX_ACTION_TIMEOUT_MILLIS);
    }

    #[test]
    fn invalid_actor_environment_falls_back_to_a_private_process_identity() {
        assert!(!valid_actor(""));
        assert!(!valid_actor("bad\nactor"));
        assert!(valid_actor("horizon:panel-1"));
        assert_eq!(BrowserController::with_actor("agent").actor, "agent");
    }

    #[test]
    fn protocol_never_exposes_a_raw_endpoint() {
        assert_eq!(protocol_kind(BackendKind::ChromiumCdp, true), ProtocolKind::Cdp);
        assert_eq!(
            protocol_kind(BackendKind::FirefoxBidi, true),
            ProtocolKind::WebDriverBidi
        );
        assert_eq!(protocol_kind(BackendKind::FirefoxBidi, false), ProtocolKind::WebDriver);
        assert_eq!(
            protocol_kind(BackendKind::SafariWebDriver, true),
            ProtocolKind::WebDriverBidi
        );
        assert_eq!(
            protocol_kind(BackendKind::SafariWebDriver, false),
            ProtocolKind::WebDriver
        );
        assert_eq!(
            ControlError::internal_io(
                "could not read browser action result",
                io::Error::new(io::ErrorKind::NotFound, "/private/runtime/result.json"),
            )
            .to_string(),
            "could not read browser action result: browser panel is not live"
        );
        assert!(!public_browser_error("protocol_error", "failed ws://127.0.0.1:9222").contains("127.0.0.1"));
    }
}
