use std::collections::BTreeSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_browser::{
    AgentActionResult, BackendKind, BrowserActionOutcome, BrowserControlAction, BrowserControlValue,
};
use horizon_core::browser::manifest;
use horizon_core::browser::manifest::{
    AgentIdentity, BrowserCreateOutcome, BrowserManifest, BrowserVisibilityOutcome, HOST_INSTANCE_ENV,
};
use thiserror::Error;

use crate::model::{BrowserPanel, ProtocolKind};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);
const RESULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
pub(crate) const DEFAULT_ACTION_TIMEOUT_MILLIS: u64 = 15_000;
pub(crate) const MAX_ACTION_TIMEOUT_MILLIS: u64 = 60_000;
/// Extra time this server waits beyond an engine-enforced bound: one
/// coordination tick (250 ms) for the engine to notice the bound, plus result
/// delivery through the manifest's locked, durably written files, which on a
/// host under heavy load with a nearly full disk has been measured at up to
/// about 2.5 s after the engine completed the action. The caller only waits
/// this long when the engine's typed outcome is late; a normal outcome
/// returns as soon as it is written.
pub(crate) const RESULT_DELIVERY_HEADROOM_MILLIS: u64 = 5_000;
const DEFAULT_CREATE_TIMEOUT_MILLIS: u64 = 60_000;
const MIN_CREATE_TIMEOUT_MILLIS: u64 = 5_000;

#[derive(Clone, Debug)]
pub(crate) struct BrowserController {
    actor: String,
    /// The Horizon host process that injected `actor`, when its launcher
    /// forwarded it. Workspace stamps only match identities from that host.
    host_instance: Option<String>,
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
    /// `None` when the host predates startup readiness.
    pub(crate) navigation: Option<horizon_core::browser::manifest::CreateNavigation>,
    pub(crate) navigation_error: Option<String>,
    pub(crate) startup_millis: u64,
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
    #[error(
        "browser panel {panel_id} is outside the calling agent's Horizon workspace; use browser_list to find controllable panels or browser_create to open one there"
    )]
    PanelOutsideWorkspace { panel_id: String },
    #[error(
        "the calling agent's Horizon host instance is unknown; Horizon injects {HOST_INSTANCE_ENV} next to HORIZON_BROWSER_ACTOR and both must reach this MCP server"
    )]
    HostInstanceUnavailable,
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
        let actor = std::env::var("HORIZON_BROWSER_ACTOR")
            .ok()
            .filter(|value| valid_actor(value));
        let host_instance = std::env::var(HOST_INSTANCE_ENV)
            .ok()
            .filter(|value| manifest::valid_host_instance(value));
        actor.map_or_else(
            || Self {
                actor: fallback.clone(),
                host_instance: None,
                process_local_ownership: Some(Arc::new(ProcessLocalOwnership::new(fallback))),
            },
            |actor| Self {
                actor,
                host_instance,
                process_local_ownership: None,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn with_actor(actor: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            host_instance: None,
            process_local_ownership: None,
        }
    }

    fn identity(&self) -> AgentIdentity<'_> {
        AgentIdentity::new(&self.actor, self.host_instance.as_deref())
    }

    /// A Horizon agent whose launcher dropped the host instance can never
    /// match a stamp; say so instead of reporting every panel as foreign.
    fn require_host_instance(&self) -> Result<(), ControlError> {
        if self.identity().workspace_scoped() && self.host_instance.is_none() {
            return Err(ControlError::HostInstanceUnavailable);
        }
        Ok(())
    }

    /// Live panels the calling identity may control: for a Horizon agent,
    /// exactly the panels its host placed in the agent's current workspace.
    pub(crate) fn list_panels(&self) -> Result<Vec<BrowserPanel>, ControlError> {
        self.require_host_instance()?;
        let mut panels = self
            .workspace_manifests()
            .map(|manifest| BrowserPanel::from_manifest(manifest, &self.actor))
            .collect::<Vec<_>>();
        panels.sort_by(|left, right| left.panel_id.cmp(&right.panel_id));
        Ok(panels)
    }

    pub(crate) fn panel(&self, panel_id: &str) -> Result<BrowserPanel, ControlError> {
        self.authorized_manifest(panel_id)
            .map(|manifest| BrowserPanel::from_manifest(manifest, &self.actor))
    }

    fn workspace_manifests(&self) -> impl Iterator<Item = BrowserManifest> {
        let directory = manifest::default_manifest_dir();
        manifest::list_panels_in(&directory)
            .into_iter()
            .filter_map(|panel_id| manifest::read(&panel_id))
            .filter(|manifest| manifest.permits(self.identity()))
    }

    /// Every control path starts here, so an out-of-workspace panel is
    /// rejected before any ownership claim, even when it is unowned. The
    /// locked manifest transactions repeat the check, so this early read is
    /// only a fast path with a clear error, never the boundary itself.
    fn authorized_manifest(&self, panel_id: &str) -> Result<BrowserManifest, ControlError> {
        self.require_host_instance()?;
        let manifest = manifest::read(panel_id).ok_or_else(|| {
            ControlError::internal_io(
                "could not read browser panel",
                io::Error::new(io::ErrorKind::NotFound, "browser manifest missing"),
            )
        })?;
        if manifest.permits(self.identity()) {
            Ok(manifest)
        } else {
            Err(ControlError::PanelOutsideWorkspace {
                panel_id: panel_id.to_string(),
            })
        }
    }

    /// Classify a refused locked transaction: a panel that no longer permits
    /// this identity was moved out of the workspace after the fast-path check,
    /// which deserves the workspace error rather than an ownership one.
    fn denied(&self, panel_id: &str, operation: &'static str, source: io::Error) -> ControlError {
        if source.kind() == io::ErrorKind::PermissionDenied
            && manifest::read(panel_id).is_some_and(|manifest| !manifest.permits(self.identity()))
        {
            return ControlError::PanelOutsideWorkspace {
                panel_id: panel_id.to_string(),
            };
        }
        ControlError::internal_io(operation, source)
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
        self.require_host_instance()?;
        if !allow_additional && let Some(panel_id) = self.existing_actor_panel_id() {
            return Err(ControlError::AdditionalPanelRequiresOptIn { panel_id });
        }
        let timeout_millis = bounded_create_timeout(timeout_millis);
        let action_id = manifest::enqueue_create(
            self.identity(),
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
                    BrowserCreateOutcome::Ready {
                        panel_local_id,
                        navigation,
                        navigation_error,
                        startup_millis,
                    } => {
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
                            navigation,
                            navigation_error,
                            startup_millis,
                        })
                    }
                    BrowserCreateOutcome::Failed { code, message } => Err(ControlError::Browser {
                        action_id,
                        code,
                        message,
                    }),
                };
            }
            // The host decides readiness by the request's own deadline; the
            // result then still travels through the locked manifest files,
            // so give its delivery the same headroom as engine-bounded actions.
            if started.elapsed() >= Duration::from_millis(timeout_millis + RESULT_DELIVERY_HEADROOM_MILLIS) {
                return Err(ControlError::CreateTimeout {
                    action_id,
                    timeout_millis,
                });
            }
            tokio::time::sleep(RESULT_POLL_INTERVAL).await;
        }
    }

    fn existing_actor_panel_id(&self) -> Option<String> {
        self.workspace_manifests()
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
        let action_id = manifest::enqueue_visibility(
            self.identity(),
            panel_id,
            visible,
            Duration::from_millis(timeout_millis),
        )
        .map_err(|source| self.denied(panel_id, "could not queue browser visibility change", source))?;
        let started = Instant::now();
        let mut last_heartbeat = started;
        loop {
            if let Some(result) = manifest::take_visibility_result(&action_id, &self.actor)
                .map_err(|source| ControlError::internal_io("could not read browser visibility result", source))?
            {
                // Gate both outcomes: a failure is panel-specific data too.
                self.refresh_claim(panel_id)?;
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
        let action_id = manifest::enqueue_action(panel_id, self.identity(), action)
            .map_err(|source| self.denied(panel_id, "could not queue browser action", source))?;
        self.wait_for_result(panel_id, action_id, timeout_millis).await
    }

    /// Execute an action whose bound the engine enforces itself (navigation
    /// and selector waits): the engine gets the full documented bound and
    /// this server waits `RESULT_DELIVERY_HEADROOM_MILLIS` longer, so the
    /// engine's typed timeout outcome always arrives before the server's own
    /// generic timeout.
    pub(crate) async fn execute_engine_bounded(
        &self,
        panel_id: &str,
        action: BrowserControlAction,
        engine_bound_millis: u64,
    ) -> Result<ActionReceipt, ControlError> {
        let engine_bound_millis = engine_bound_millis.clamp(1, MAX_ACTION_TIMEOUT_MILLIS);
        self.ensure_claim(panel_id)?;
        let action_id = manifest::enqueue_action(panel_id, self.identity(), action)
            .map_err(|source| self.denied(panel_id, "could not queue browser action", source))?;
        self.wait_for_engine_result(panel_id, action_id, engine_bound_millis)
            .await
    }

    /// Like `wait_for_result`, but for an action whose bound the engine
    /// enforces itself (navigations and selector waits). The engine always
    /// produces a typed result for such an action: at its bound when it is
    /// being observed, and at once when it is dispatched late because the
    /// bound is anchored at the request time. Only the driver being blocked
    /// (a synchronous backend call, a classic navigation, a claimed batch
    /// serviced in order) can delay that result, so this keeps waiting for
    /// it past the bound plus delivery headroom, up to a hard cap, instead of
    /// returning a generic timeout that hides the typed outcome.
    async fn wait_for_engine_result(
        &self,
        panel_id: &str,
        action_id: String,
        engine_bound_millis: u64,
    ) -> Result<ActionReceipt, ControlError> {
        let started = Instant::now();
        // What the controller actually waits: the engine bound, the delivery
        // headroom, and the allowance for a blocked driver; the error reports
        // this whole duration.
        let timeout_millis = engine_bound_millis + RESULT_DELIVERY_HEADROOM_MILLIS + MAX_ACTION_TIMEOUT_MILLIS;
        let hard_cap = Duration::from_millis(timeout_millis);
        let mut last_heartbeat = started;
        loop {
            if let Some(result) = poll_action_result(panel_id, &action_id)? {
                return finish_polled_result(result, || self.refresh_claim(panel_id));
            }
            if started.elapsed() >= hard_cap {
                if let Some(result) = poll_action_result(panel_id, &action_id)? {
                    return finish_polled_result(result, || self.refresh_claim(panel_id));
                }
                return Err(ControlError::Timeout {
                    action_id,
                    timeout_millis,
                });
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                if let Err(refresh_error) = self.refresh_claim(panel_id) {
                    let result = poll_action_result(panel_id, &action_id)?;
                    return finish_repolled_result(result, refresh_error);
                }
                last_heartbeat = Instant::now();
            }
            tokio::time::sleep(RESULT_POLL_INTERVAL).await;
        }
    }

    pub(crate) fn request_handoff(&self, panel_id: &str, reason: &str) -> Result<String, ControlError> {
        self.ensure_claim(panel_id)?;
        manifest::request_handoff(panel_id, self.identity(), reason)
            .map_err(|source| self.denied(panel_id, "could not request browser handoff", source))
    }

    pub(crate) fn read_audit(&self, panel_id: &str) -> Result<Vec<horizon_browser::BrowserAuditEntry>, ControlError> {
        self.authorized_manifest(panel_id)?;
        tracing::debug!(actor = %self.actor, panel_id, "reading browser action audit");
        manifest::read_audit_for(panel_id, self.identity())
            .map_err(|source| self.denied(panel_id, "could not read browser audit", source))
    }

    fn ensure_claim(&self, panel_id: &str) -> Result<(), ControlError> {
        self.authorized_manifest(panel_id)?;
        let result = match manifest::heartbeat(panel_id, self.identity()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                manifest::claim(panel_id, self.identity(), None)
                    .map_err(|source| self.denied(panel_id, "could not claim browser panel", source))
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
        manifest::heartbeat(panel_id, self.identity())
            .map_err(|source| self.denied(panel_id, "lost browser ownership while waiting", source))
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
            if let Some(result) = poll_action_result(panel_id, &action_id)? {
                return finish_polled_result(result, || self.refresh_claim(panel_id));
            }
            if started.elapsed() >= timeout {
                if let Some(result) = poll_action_result(panel_id, &action_id)? {
                    return finish_polled_result(result, || self.refresh_claim(panel_id));
                }
                return Err(ControlError::Timeout {
                    action_id,
                    timeout_millis,
                });
            }
            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                if let Err(refresh_error) = self.refresh_claim(panel_id) {
                    let result = poll_action_result(panel_id, &action_id)?;
                    return finish_repolled_result(result, refresh_error);
                }
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

    fn is_missing_panel(&self) -> bool {
        matches!(self, Self::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

fn bounded_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(DEFAULT_ACTION_TIMEOUT_MILLIS)
        .clamp(1, MAX_ACTION_TIMEOUT_MILLIS)
}

/// The per-action timeout this server enforces for `timeout_millis`.
pub(crate) fn bounded_action_timeout(timeout_millis: Option<u64>) -> u64 {
    bounded_timeout(timeout_millis)
}

fn bounded_create_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(DEFAULT_CREATE_TIMEOUT_MILLIS)
        .clamp(MIN_CREATE_TIMEOUT_MILLIS, MAX_ACTION_TIMEOUT_MILLIS)
}

fn poll_action_result(panel_id: &str, action_id: &str) -> Result<Option<AgentActionResult>, ControlError> {
    manifest::take_action_result(panel_id, action_id)
        .map_err(|source| ControlError::internal_io("could not read browser action result", source))
}

fn finish_polled_result(
    result: AgentActionResult,
    refresh_claim: impl FnOnce() -> Result<(), ControlError>,
) -> Result<ActionReceipt, ControlError> {
    // Results normally remain panel-specific data, so ownership and workspace
    // membership must still be live when they are disclosed. A data-free
    // `browser_unavailable` may outlive teardown only when refresh proves that
    // the manifest is gone; live ownership and workspace errors still win.
    let browser_unavailable = matches!(
        &result.outcome,
        BrowserActionOutcome::Failed { error } if error.code == "browser_unavailable"
    );
    if let Err(error) = refresh_claim()
        && !(browser_unavailable && error.is_missing_panel())
    {
        return Err(error);
    }
    outcome(result)
}

fn finish_repolled_result(
    result: Option<AgentActionResult>,
    refresh_error: ControlError,
) -> Result<ActionReceipt, ControlError> {
    let Some(result) = result else {
        return Err(refresh_error);
    };
    finish_polled_result(result, || Err(refresh_error))
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

/// Only identities injected by a Horizon host may ask that host for new
/// panels or visibility changes; they are also the workspace-scoped ones.
fn is_horizon_actor(actor: &str) -> bool {
    manifest::actor_is_workspace_scoped(actor)
}

fn panel_belongs_to_actor(panel: &BrowserManifest, actor: &str) -> bool {
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
    use std::cell::Cell;

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
    fn missing_manifest_refresh_is_ignored_only_for_browser_unavailable() {
        let unavailable = AgentActionResult::failed(
            "shutdown-wait",
            horizon_browser::BrowserControlFailure::new("browser_unavailable", "the browser stopped while waiting"),
        );
        let refreshed = Cell::new(false);
        let error = finish_polled_result(unavailable, || {
            refreshed.set(true);
            Err(missing_panel_error())
        })
        .unwrap_err();
        assert!(refreshed.get());
        assert!(matches!(
            error,
            ControlError::Browser { ref code, .. } if code == "browser_unavailable"
        ));

        let unavailable = AgentActionResult::failed(
            "foreign-unavailable",
            horizon_browser::BrowserControlFailure::new("browser_unavailable", "backend stopped"),
        );
        let error = finish_polled_result(unavailable, || {
            Err(ControlError::PanelOutsideWorkspace {
                panel_id: "foreign-panel".to_string(),
            })
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ControlError::PanelOutsideWorkspace { ref panel_id } if panel_id == "foreign-panel"
        ));

        let unavailable = AgentActionResult::failed(
            "lost-ownership",
            horizon_browser::BrowserControlFailure::new("browser_unavailable", "backend stopped"),
        );
        let error = finish_polled_result(unavailable, || {
            Err(ControlError::internal_io(
                "lost browser ownership while waiting",
                io::Error::new(io::ErrorKind::PermissionDenied, "ownership changed"),
            ))
        })
        .unwrap_err();
        assert!(matches!(
            error,
            ControlError::Io { ref source, .. } if source.kind() == io::ErrorKind::PermissionDenied
        ));

        let completed = AgentActionResult::completed("completed", BrowserControlValue::Accepted);
        let refreshed = Cell::new(false);
        let receipt = finish_polled_result(completed, || {
            refreshed.set(true);
            Ok(())
        })
        .unwrap();
        assert!(refreshed.get());
        assert_eq!(receipt.action_id, "completed");

        let other_failure = AgentActionResult::failed(
            "timed-out-wait",
            horizon_browser::BrowserControlFailure::new("wait_timeout", "selector did not match"),
        );
        let refreshed = Cell::new(false);
        let error = finish_polled_result(other_failure, || {
            refreshed.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(refreshed.get());
        assert!(matches!(
            error,
            ControlError::Browser { ref code, .. } if code == "wait_timeout"
        ));
    }

    #[test]
    fn failed_heartbeat_repolls_for_a_published_shutdown_result() {
        let unavailable = AgentActionResult::failed(
            "shutdown-wait",
            horizon_browser::BrowserControlFailure::new("browser_unavailable", "the browser stopped while waiting"),
        );
        let error = finish_repolled_result(Some(unavailable), missing_panel_error()).unwrap_err();
        assert!(matches!(
            error,
            ControlError::Browser { ref code, .. } if code == "browser_unavailable"
        ));

        let error = finish_repolled_result(None, missing_panel_error()).unwrap_err();
        assert!(error.is_missing_panel());

        let completed = AgentActionResult::completed("completed", BrowserControlValue::Accepted);
        let error = finish_repolled_result(Some(completed), missing_panel_error()).unwrap_err();
        assert!(error.is_missing_panel());
    }

    fn missing_panel_error() -> ControlError {
        ControlError::internal_io(
            "lost browser ownership while waiting",
            io::Error::new(io::ErrorKind::NotFound, "browser manifest missing"),
        )
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
        assert!(controller.host_instance.is_none());
        assert!(
            controller.require_host_instance().is_ok(),
            "external identities need no host"
        );
        assert!(matches!(
            BrowserController::with_actor("horizon:agent-a").require_host_instance(),
            Err(ControlError::HostInstanceUnavailable)
        ));
    }

    #[test]
    fn workspace_membership_gates_horizon_actors_and_fails_closed() {
        let mut panel = BrowserManifest {
            panel_local_id: "browser-panel".to_string(),
            owner: Some(manifest::ManifestOwner {
                name: "horizon:stale-owner".to_string(),
                tty: None,
                updated_at: 1,
            }),
            ..BrowserManifest::default()
        };
        let member = AgentIdentity::new("horizon:agent-a", Some("host-a"));
        assert!(!panel.permits(member), "unstamped manifests fail closed");
        assert!(
            panel.permits(AgentIdentity::new("horizon-mcp:4242", None)),
            "external identities stay unscoped"
        );

        panel.workspace = Some(manifest::ManifestWorkspace::new(
            "host-a",
            "workspace-a",
            vec!["horizon:agent-a".to_string()],
        ));
        assert!(
            !panel.permits(member),
            "a stamp without a matching driver host is stale"
        );
        panel.host = Some("host-a".to_string());
        assert!(panel.permits(member));
        assert!(
            !panel.permits(AgentIdentity::new("horizon:agent-b", Some("host-a"))),
            "a stale owner does not open the panel to another workspace"
        );
        assert!(
            !panel.permits(AgentIdentity::new("horizon:agent-a", Some("host-b"))),
            "the same persisted agent id in another live host is a different agent"
        );
        assert!(!panel.permits(AgentIdentity::new("horizon:stale-owner", Some("host-a"))));
        let controller = BrowserController::with_actor("horizon-mcp:4242");
        assert!(matches!(
            controller.denied(
                "missing-panel",
                "could not queue browser action",
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "agent does not have a live ownership claim"
                ),
            ),
            ControlError::Io { .. }
        ));
        assert_eq!(
            ControlError::PanelOutsideWorkspace {
                panel_id: "browser-panel".to_string(),
            }
            .to_string(),
            "browser panel browser-panel is outside the calling agent's Horizon workspace; use browser_list to find controllable panels or browser_create to open one there"
        );
    }

    #[test]
    fn existing_panel_guard_remembers_the_creating_actor_after_heartbeat_expiry() {
        let panel = BrowserManifest {
            panel_local_id: "browser-panel".to_string(),
            owner: Some(manifest::ManifestOwner {
                name: "horizon:agent-panel".to_string(),
                tty: None,
                updated_at: 1,
            }),
            ..BrowserManifest::default()
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
