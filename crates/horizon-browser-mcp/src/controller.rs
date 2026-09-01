use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::{
    AgentActionResult, BackendKind, BrowserActionOutcome, BrowserControlAction, BrowserControlValue,
};
use horizon_core::browser::manifest;
use horizon_core::browser::manifest::{BrowserCreateOutcome, BrowserVisibilityOutcome};
use thiserror::Error;

use crate::model::{BrowserPanel, ProtocolKind};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const RESULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
pub(crate) const DEFAULT_ACTION_TIMEOUT_MILLIS: u64 = 15_000;
pub(crate) const MAX_ACTION_TIMEOUT_MILLIS: u64 = 60_000;
const DEFAULT_CREATE_TIMEOUT_MILLIS: u64 = 60_000;
const MIN_CREATE_TIMEOUT_MILLIS: u64 = 5_000;

#[derive(Clone, Debug)]
pub(crate) struct BrowserController {
    actor: String,
    host_instance_id: Option<String>,
    process_local_ownership: Option<Arc<ProcessLocalOwnership>>,
}

#[derive(Debug)]
struct ProcessLocalOwnership {
    actor: String,
    panel_ids: Mutex<BTreeSet<String>>,
}

impl ProcessLocalOwnership {
    fn new(actor: String) -> Self {
        Self {
            actor,
            panel_ids: Mutex::new(BTreeSet::new()),
        }
    }

    fn remember(&self, panel_id: &str) {
        let mut panel_ids = self.panel_ids.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !panel_ids.contains(panel_id) {
            panel_ids.insert(panel_id.to_string());
        }
    }
}

impl Drop for ProcessLocalOwnership {
    fn drop(&mut self) {
        let panel_ids = self
            .panel_ids
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for panel_id in std::mem::take(panel_ids) {
            match manifest::release(&panel_id, &self.actor) {
                Ok(true) => tracing::debug!(actor = %self.actor, panel_id, "released process-local browser ownership"),
                Ok(false) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(actor = %self.actor, panel_id, %error, "could not release process-local browser ownership");
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct ActionReceipt {
    pub(crate) action_id: String,
    pub(crate) value: BrowserControlValue,
}

#[derive(Debug)]
pub(crate) struct CreateReceipt {
    pub(crate) action_id: String,
    pub(crate) panel: BrowserPanel,
}

#[derive(Debug)]
pub(crate) struct VisibilityReceipt {
    pub(crate) action_id: String,
    pub(crate) panel: BrowserPanel,
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
    #[error(
        "browser create request {action_id} timed out after {timeout_millis} ms; call browser_list before retrying because a late panel may still be visible"
    )]
    CreateTimeout { action_id: String, timeout_millis: u64 },
    #[error("browser_create is available only to an agent panel launched inside Horizon")]
    CreateUnavailable,
    #[error("browser workspace scope is unavailable; restart the calling Horizon agent panel")]
    WorkspaceScopeUnavailable,
    #[error("browser panel {panel_id} is not in the calling agent's Horizon workspace")]
    PanelOutsideWorkspace { panel_id: String },
    #[error(
        "browser panel {panel_id} is already owned by this agent; reuse it, or set allow_additional=true only when the user explicitly requested an independent browser session"
    )]
    AdditionalPanelRequiresOptIn { panel_id: String },
    #[error("browser visibility can be changed only by an agent panel launched inside Horizon")]
    VisibilityUnavailable,
    #[error(
        "browser visibility request {action_id} timed out after {timeout_millis} ms; call browser_panel before retrying because the change may have completed late"
    )]
    VisibilityTimeout { action_id: String, timeout_millis: u64 },
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
        let actor = std::env::var("HORIZON_BROWSER_ACTOR").ok();
        let host_instance_id = std::env::var(manifest::HOST_INSTANCE_ENV).ok();
        Self::from_environment_parts(actor, host_instance_id, std::env::var_os("HORIZON").is_some(), fallback)
    }

    fn from_environment_parts(
        actor: Option<String>,
        host_instance_id: Option<String>,
        horizon_marker: bool,
        fallback: String,
    ) -> Self {
        let horizon_scoped =
            horizon_marker || host_instance_id.is_some() || actor.as_deref().is_some_and(is_horizon_actor);
        let actor = actor.filter(|value| valid_actor(value));
        let host_instance_id = host_instance_id.filter(|value| valid_host_instance_id(value));
        if horizon_scoped && actor.as_deref().is_none_or(|value| !is_horizon_actor(value)) {
            return Self {
                actor: format!("horizon:unavailable:{fallback}"),
                host_instance_id: None,
                process_local_ownership: None,
            };
        }
        match actor {
            Some(actor) => Self {
                actor,
                host_instance_id,
                process_local_ownership: None,
            },
            None => Self::standalone_with_actor(fallback),
        }
    }

    pub(crate) fn standalone() -> Self {
        Self::standalone_with_actor(format!("horizon-mcp:{}", std::process::id()))
    }

    fn standalone_with_actor(actor: String) -> Self {
        Self {
            actor: actor.clone(),
            host_instance_id: None,
            process_local_ownership: Some(Arc::new(ProcessLocalOwnership::new(actor))),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_actor(actor: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            host_instance_id: None,
            process_local_ownership: None,
        }
    }

    pub(crate) fn list_panels(&self) -> Vec<BrowserPanel> {
        let directory = manifest::default_manifest_dir();
        self.panels_from(
            manifest::list_panels_in(&directory)
                .into_iter()
                .filter_map(|panel_id| manifest::read(&panel_id)),
        )
    }

    fn panels_from(&self, manifests: impl IntoIterator<Item = manifest::BrowserManifest>) -> Vec<BrowserPanel> {
        let mut panels = manifests
            .into_iter()
            .filter(|panel| self.can_access_manifest(panel))
            .map(|panel| BrowserPanel::from_manifest(panel, &self.actor))
            .collect::<Vec<_>>();
        panels.sort_by(|left, right| left.panel_id.cmp(&right.panel_id));
        panels
    }

    pub(crate) fn panel(&self, panel_id: &str) -> Result<BrowserPanel, ControlError> {
        let panel = manifest::read(panel_id).ok_or_else(|| {
            ControlError::internal_io(
                "could not read browser panel",
                io::Error::new(io::ErrorKind::NotFound, "browser panel is not live"),
            )
        })?;
        self.panel_from_manifest(panel)
    }

    fn panel_from_manifest(&self, panel: manifest::BrowserManifest) -> Result<BrowserPanel, ControlError> {
        if self.can_access_manifest(&panel) {
            Ok(BrowserPanel::from_manifest(panel, &self.actor))
        } else {
            Err(ControlError::PanelOutsideWorkspace {
                panel_id: panel.panel_local_id,
            })
        }
    }

    pub(crate) async fn create(
        &self,
        url: Option<String>,
        backend: Option<BackendKind>,
        visible: bool,
        allow_additional: bool,
        timeout_millis: Option<u64>,
    ) -> Result<CreateReceipt, ControlError> {
        if !is_horizon_actor(&self.actor) {
            return Err(ControlError::CreateUnavailable);
        }
        self.ensure_workspace_scope_available()?;
        if !allow_additional && let Some(panel_id) = self.existing_actor_panel_id() {
            return Err(ControlError::AdditionalPanelRequiresOptIn { panel_id });
        }
        let timeout_millis = bounded_create_timeout(timeout_millis);
        let action_id = manifest::enqueue_create(
            &self.actor,
            url,
            backend,
            visible,
            Duration::from_millis(timeout_millis),
        )
        .map_err(|source| ControlError::internal_io("could not queue browser panel creation", source))?;
        let started = Instant::now();
        loop {
            if let Some(result) = manifest::take_create_result(&action_id, &self.actor)
                .map_err(|source| ControlError::internal_io("could not read browser create result", source))?
            {
                return match result.outcome {
                    BrowserCreateOutcome::Ready { panel_local_id } => {
                        self.ensure_claim(&panel_local_id)?;
                        let manifest = manifest::read(&panel_local_id).ok_or_else(|| {
                            ControlError::internal_io(
                                "could not read created browser panel",
                                io::Error::new(io::ErrorKind::NotFound, "created browser manifest disappeared"),
                            )
                        })?;
                        Ok(CreateReceipt {
                            action_id,
                            panel: BrowserPanel::from_manifest(manifest, &self.actor),
                        })
                    }
                    BrowserCreateOutcome::Failed { code, message } => Err(ControlError::Browser {
                        action_id,
                        code,
                        message,
                    }),
                };
            }
            if started.elapsed() >= Duration::from_millis(timeout_millis) {
                return Err(ControlError::CreateTimeout {
                    action_id,
                    timeout_millis,
                });
            }
            tokio::time::sleep(RESULT_POLL_INTERVAL).await;
        }
    }

    fn existing_actor_panel_id(&self) -> Option<String> {
        let directory = manifest::default_manifest_dir();
        manifest::list_panels_in(&directory)
            .into_iter()
            .filter_map(|panel_id| manifest::read(&panel_id))
            .filter(|panel| self.can_access_manifest(panel))
            .find(|panel| panel_belongs_to_actor(panel, &self.actor))
            .map(|panel| panel.panel_local_id)
    }

    pub(crate) async fn set_visibility(
        &self,
        panel_id: &str,
        visible: bool,
        timeout_millis: Option<u64>,
    ) -> Result<VisibilityReceipt, ControlError> {
        if !is_horizon_actor(&self.actor) {
            return Err(ControlError::VisibilityUnavailable);
        }
        let timeout_millis = bounded_timeout(timeout_millis);
        self.ensure_claim(panel_id)?;
        let action_id =
            manifest::enqueue_visibility(&self.actor, panel_id, visible, Duration::from_millis(timeout_millis))
                .map_err(|source| ControlError::internal_io("could not queue browser visibility change", source))?;
        let started = Instant::now();
        let mut last_heartbeat = started;
        loop {
            if let Some(result) = manifest::take_visibility_result(&action_id, &self.actor)
                .map_err(|source| ControlError::internal_io("could not read browser visibility result", source))?
            {
                return match result.outcome {
                    BrowserVisibilityOutcome::Ready { visible: actual } => {
                        let manifest = manifest::read(panel_id).ok_or_else(|| {
                            ControlError::internal_io(
                                "could not read updated browser panel",
                                io::Error::new(io::ErrorKind::NotFound, "browser manifest disappeared"),
                            )
                        })?;
                        if actual != visible || manifest.hidden == visible {
                            return Err(ControlError::internal_io(
                                "could not verify browser visibility",
                                io::Error::new(io::ErrorKind::InvalidData, "visibility result did not match request"),
                            ));
                        }
                        Ok(VisibilityReceipt {
                            action_id,
                            panel: BrowserPanel::from_manifest(manifest, &self.actor),
                        })
                    }
                    BrowserVisibilityOutcome::Failed { code, message } => Err(ControlError::Browser {
                        action_id,
                        code,
                        message,
                    }),
                };
            }
            if started.elapsed() >= Duration::from_millis(timeout_millis) {
                return Err(ControlError::VisibilityTimeout {
                    action_id,
                    timeout_millis,
                });
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                self.refresh_claim(panel_id)?;
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(RESULT_POLL_INTERVAL).await;
        }
    }

    pub(crate) async fn execute(
        &self,
        panel_id: &str,
        action: BrowserControlAction,
        timeout_millis: Option<u64>,
    ) -> Result<ActionReceipt, ControlError> {
        let timeout_millis = bounded_timeout(timeout_millis);
        self.ensure_claim(panel_id)?;
        let action_id = manifest::enqueue_action(panel_id, &self.actor, action, self.host_instance_id.as_deref())
            .map_err(|source| ControlError::internal_io("could not queue browser action", source))?;
        self.wait_for_result(panel_id, action_id, timeout_millis).await
    }

    pub(crate) fn request_handoff(&self, panel_id: &str, reason: &str) -> Result<String, ControlError> {
        self.ensure_claim(panel_id)?;
        manifest::request_handoff(panel_id, &self.actor, reason, self.host_instance_id.as_deref())
            .map_err(|source| ControlError::internal_io("could not request browser handoff", source))
    }

    pub(crate) fn read_audit(&self, panel_id: &str) -> Result<Vec<horizon_browser::BrowserAuditEntry>, ControlError> {
        self.ensure_panel_in_workspace(panel_id)?;
        tracing::debug!(actor = %self.actor, panel_id, "reading browser action audit");
        manifest::read_audit_for_actor(panel_id, &self.actor, self.host_instance_id.as_deref())
            .map_err(|source| ControlError::internal_io("could not read browser audit", source))
    }

    fn ensure_claim(&self, panel_id: &str) -> Result<(), ControlError> {
        self.ensure_panel_in_workspace(panel_id)?;
        let result = match manifest::heartbeat(panel_id, &self.actor, self.host_instance_id.as_deref()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                manifest::claim(panel_id, &self.actor, None, self.host_instance_id.as_deref())
                    .map_err(|source| ControlError::internal_io("could not claim browser panel", source))
            }
            Err(source) => Err(ControlError::internal_io("could not refresh browser ownership", source)),
        };
        if result.is_ok()
            && let Some(ownership) = self.process_local_ownership.as_deref()
        {
            ownership.remember(panel_id);
        }
        result
    }

    pub(crate) fn refresh_claim(&self, panel_id: &str) -> Result<(), ControlError> {
        self.ensure_panel_in_workspace(panel_id)?;
        manifest::heartbeat(panel_id, &self.actor, self.host_instance_id.as_deref())
            .map_err(|source| ControlError::internal_io("lost browser ownership while waiting", source))
    }

    fn ensure_workspace_scope_available(&self) -> Result<(), ControlError> {
        if is_horizon_actor(&self.actor) && self.host_instance_id.is_none() {
            return Err(ControlError::WorkspaceScopeUnavailable);
        }
        Ok(())
    }

    pub(crate) fn ensure_panel_in_workspace(&self, panel_id: &str) -> Result<(), ControlError> {
        if !is_horizon_actor(&self.actor) {
            return Ok(());
        }
        self.ensure_workspace_scope_available()?;
        let panel = manifest::read(panel_id).ok_or_else(|| {
            ControlError::internal_io(
                "could not read browser panel workspace scope",
                io::Error::new(io::ErrorKind::NotFound, "browser panel is not live"),
            )
        })?;
        if self.can_access_manifest(&panel) {
            Ok(())
        } else {
            Err(ControlError::PanelOutsideWorkspace {
                panel_id: panel_id.to_string(),
            })
        }
    }

    fn can_access_manifest(&self, panel: &manifest::BrowserManifest) -> bool {
        if !is_horizon_actor(&self.actor) {
            return true;
        }
        panel.permits_actor(&self.actor, self.host_instance_id.as_deref())
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
                self.refresh_claim(panel_id)?;
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

fn bounded_create_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(DEFAULT_CREATE_TIMEOUT_MILLIS)
        .clamp(MIN_CREATE_TIMEOUT_MILLIS, MAX_ACTION_TIMEOUT_MILLIS)
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

fn valid_host_instance_id(host_instance_id: &str) -> bool {
    !host_instance_id.trim().is_empty()
        && host_instance_id.len() <= 128
        && !host_instance_id.chars().any(char::is_control)
}

fn is_horizon_actor(actor: &str) -> bool {
    actor
        .strip_prefix("horizon:")
        .is_some_and(|identity| !identity.is_empty())
}

fn panel_belongs_to_actor(panel: &manifest::BrowserManifest, actor: &str) -> bool {
    panel.owner.as_ref().is_some_and(|owner| owner.name == actor)
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
        assert_eq!(bounded_create_timeout(None), DEFAULT_CREATE_TIMEOUT_MILLIS);
        assert_eq!(bounded_create_timeout(Some(0)), MIN_CREATE_TIMEOUT_MILLIS);
        assert_eq!(bounded_create_timeout(Some(u64::MAX)), MAX_ACTION_TIMEOUT_MILLIS);
    }

    #[test]
    fn invalid_actor_environment_falls_back_to_a_private_process_identity() {
        assert!(!valid_actor(""));
        assert!(!valid_actor("bad\nactor"));
        assert!(valid_actor("horizon:panel-1"));
        assert!(is_horizon_actor("horizon:panel-1"));
        assert!(!is_horizon_actor("horizon:"));
        assert!(!is_horizon_actor("agent"));
        let controller = BrowserController::with_actor("agent");
        assert_eq!(controller.actor, "agent");
        assert!(controller.host_instance_id.is_none());
    }

    #[test]
    fn malformed_horizon_environment_fails_closed_without_changing_standalone_fallback() {
        let unavailable = BrowserController::from_environment_parts(
            Some("not-a-horizon-actor".to_string()),
            Some("host-a".to_string()),
            true,
            "horizon-mcp:test".to_string(),
        );
        assert!(is_horizon_actor(&unavailable.actor));
        assert!(unavailable.host_instance_id.is_none());
        assert!(unavailable.process_local_ownership.is_none());

        let standalone = BrowserController::from_environment_parts(None, None, false, "horizon-mcp:test".to_string());
        assert_eq!(standalone.actor, "horizon-mcp:test");
        assert!(standalone.process_local_ownership.is_some());
    }

    #[test]
    fn discovery_and_panel_control_follow_host_workspace_and_moves() {
        let actor = "horizon:host-a:agent-a";
        let controller = BrowserController {
            actor: actor.to_string(),
            host_instance_id: Some("host-a".to_string()),
            process_local_ownership: None,
        };
        let scoped = |panel: &str, host: &str, actors: Vec<&str>| manifest::BrowserManifest {
            panel_local_id: panel.to_string(),
            workspace_scope: Some(manifest::ManifestWorkspaceScope {
                host_instance_id: host.to_string(),
                workspace_local_id: "same-local-id".to_string(),
                actors: actors.into_iter().map(str::to_string).collect(),
            }),
            ..manifest::BrowserManifest::default()
        };
        let mut same_workspace = scoped("same", "host-a", vec![actor]);
        same_workspace.owner = Some(manifest::ManifestOwner {
            name: "horizon:host-a:stale".to_string(),
            tty: None,
            updated_at: 1,
        });
        let other_workspace = scoped("other-workspace", "host-a", vec!["horizon:host-a:agent-b"]);
        let other_host = scoped("other-host", "host-b", vec![actor]);
        let legacy = manifest::BrowserManifest {
            panel_local_id: "legacy".to_string(),
            ..manifest::BrowserManifest::default()
        };

        assert_eq!(
            controller
                .panels_from([
                    same_workspace.clone(),
                    other_workspace.clone(),
                    other_host.clone(),
                    legacy.clone()
                ])
                .into_iter()
                .map(|panel| panel.panel_id)
                .collect::<Vec<_>>(),
            ["same"]
        );
        assert!(controller.panel_from_manifest(same_workspace.clone()).is_ok());
        for panel in [other_workspace, other_host, legacy] {
            assert!(matches!(
                controller.panel_from_manifest(panel),
                Err(ControlError::PanelOutsideWorkspace { .. })
            ));
        }

        same_workspace.workspace_scope.as_mut().unwrap().actors = vec!["horizon:host-a:agent-b".to_string()];
        assert!(controller.panels_from([same_workspace.clone()]).is_empty());
        assert!(matches!(
            controller.panel_from_manifest(same_workspace),
            Err(ControlError::PanelOutsideWorkspace { .. })
        ));
    }

    #[test]
    fn existing_panel_guard_remembers_the_creating_actor_after_heartbeat_expiry() {
        let panel = manifest::BrowserManifest {
            panel_local_id: "browser-panel".to_string(),
            owner: Some(manifest::ManifestOwner {
                name: "horizon:agent-panel".to_string(),
                tty: None,
                updated_at: 1,
            }),
            ..manifest::BrowserManifest::default()
        };

        assert!(panel_belongs_to_actor(&panel, "horizon:agent-panel"));
        assert!(!panel_belongs_to_actor(&panel, "horizon:other-panel"));
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
