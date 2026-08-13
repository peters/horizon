use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, AgentSessionKey, Board, PanelId, PanelKind, PanelResume,
    live_claude_session_ids,
};

use super::util::{empty_string_as_none, short_session_id, truncate_session_label};
use super::{ActiveSession, DetachedWorkspaceViewportState, HorizonApp, ResolvedSession};

const SESSION_BINDING_ACTIVITY_WINDOW: Duration = Duration::from_secs(10);
const STARTUP_BOOTSTRAP_FAILURE_REPAINT_INTERVAL: Duration = Duration::from_secs(1);

mod loading;
mod types;

pub(super) use loading::render_loading_view;
pub(super) use types::{
    StartupBootstrap, StartupBootstrapFailure, StartupBootstrapOutcome, StartupBootstrapValidationFailure,
};

#[derive(Clone)]
struct DynamicPanelBindingState {
    panel_id: PanelId,
    kind: PanelKind,
    cwd: String,
    launched_at_millis: i64,
    session_binding: Option<AgentSessionBinding>,
    recent_output: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartupBootstrapFailureAction {
    Retry,
    ContinueWithoutExactResumes,
    OpenWithoutSaving,
}

fn collect_dynamic_binding_updates(
    dynamic_panels: &[DynamicPanelBindingState],
    reserved_session_keys: &HashSet<AgentSessionKey>,
    recent_for: impl Fn(PanelKind, Option<&str>) -> Vec<horizon_core::AgentSessionRecord>,
) -> Vec<(PanelId, AgentSessionBinding)> {
    let mut used_session_keys = reserved_session_keys.clone();
    used_session_keys.extend(dynamic_panels.iter().filter_map(|panel| {
        panel
            .session_binding
            .as_ref()
            .map(|binding| AgentSessionKey::new(panel.kind, &binding.session_id))
    }));

    let mut grouped_panels: HashMap<(PanelKind, String), Vec<&DynamicPanelBindingState>> = HashMap::new();
    for panel in dynamic_panels {
        grouped_panels
            .entry((panel.kind, panel.cwd.clone()))
            .or_default()
            .push(panel);
    }

    let mut ordered_groups: Vec<_> = grouped_panels.into_iter().collect();
    ordered_groups.sort_by(|((left_kind, left_cwd), _), ((right_kind, right_cwd), _)| {
        left_cwd
            .is_empty()
            .cmp(&right_cwd.is_empty())
            .then_with(|| left_kind.display_name().cmp(right_kind.display_name()))
            .then_with(|| left_cwd.cmp(right_cwd))
    });

    let mut assignments = Vec::new();
    for ((kind, cwd), panels) in ordered_groups {
        if kind == PanelKind::Claude {
            continue;
        }

        let mut candidates = recent_for(kind, empty_string_as_none(&cwd));
        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.updated_at));

        let active_bound_panels: Vec<_> = panels
            .iter()
            .filter(|panel| panel.recent_output && panel.session_binding.is_some())
            .collect();
        if active_bound_panels.len() == 1 {
            let panel = active_bound_panels[0];
            let Some(current_binding) = panel.session_binding.as_ref() else {
                continue;
            };
            if let Some(candidate) = candidates.iter().find(|candidate| {
                candidate.session_id != current_binding.session_id
                    && candidate.updated_at > current_binding.updated_at.unwrap_or(0)
                    && !used_session_keys.contains(&AgentSessionKey::new(kind, &candidate.session_id))
            }) {
                used_session_keys.insert(AgentSessionKey::new(kind, &candidate.session_id));
                assignments.push((panel.panel_id, candidate.clone().into_binding()));
            }
        }

        let mut unbound_panels: Vec<_> = panels
            .iter()
            .filter(|panel| panel.session_binding.is_none())
            .copied()
            .collect();
        if unbound_panels.is_empty() {
            continue;
        }

        unbound_panels.sort_by_key(|panel| std::cmp::Reverse(panel.launched_at_millis));
        let oldest_launch = unbound_panels
            .iter()
            .map(|panel| panel.launched_at_millis)
            .min()
            .unwrap_or(0);
        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(|candidate| {
                !used_session_keys.contains(&AgentSessionKey::new(kind, &candidate.session_id))
                    && candidate.updated_at >= oldest_launch.saturating_sub(300_000)
            })
            .collect();
        for (panel, candidate) in unbound_panels.into_iter().zip(candidates) {
            used_session_keys.insert(AgentSessionKey::new(kind, &candidate.session_id));
            assignments.push((panel.panel_id, candidate.into_binding()));
        }
    }

    assignments
}

fn panel_uses_dynamic_binding(panel: &horizon_core::Panel) -> bool {
    panel.kind.supports_session_binding() && !matches!(panel.resume, PanelResume::Session { .. })
}

fn panel_session_id(panel: &horizon_core::Panel) -> Option<&str> {
    panel
        .session_binding
        .as_ref()
        .map(|binding| binding.session_id.as_str())
        .or(match &panel.resume {
            PanelResume::Session { session_id } => Some(session_id.as_str()),
            PanelResume::Fresh | PanelResume::Last => None,
        })
}

impl HorizonApp {
    pub(super) fn activate_persistent_session(&mut self, session: &ResolvedSession) {
        self.release_active_session_lease();
        self.transcript_root = Some(session.transcript_root.clone());
        self.startup_chooser = None;
        self.active_session = Some(ActiveSession {
            session_id: session.session_id.clone(),
            lease: match self.session_store.acquire_lease(&session.session_id) {
                Ok(lease) => Some(lease),
                Err(error) => {
                    tracing::warn!("failed to acquire session lease: {error}");
                    None
                }
            },
            last_lease_refresh: Some(Instant::now()),
            persistent: true,
        });
        self.apply_runtime_state(&session.runtime_state);
    }

    pub(super) fn activate_ephemeral_session(&mut self, runtime_state: &horizon_core::RuntimeState) {
        self.release_active_session_lease();
        self.active_session = Some(ActiveSession {
            session_id: "ephemeral".to_string(),
            lease: None,
            last_lease_refresh: None,
            persistent: false,
        });
        self.transcript_root = None;
        self.startup_chooser = None;
        self.apply_runtime_state(runtime_state);
    }

    pub(super) fn apply_runtime_state(&mut self, runtime_state: &horizon_core::RuntimeState) {
        self.window_config = runtime_state.window_or(&self.template_config.window).clone();
        self.detached_workspaces = runtime_state
            .detached_workspaces
            .iter()
            .filter(|workspace| !workspace.workspace_local_id.is_empty())
            .map(|workspace| {
                (
                    workspace.workspace_local_id.clone(),
                    DetachedWorkspaceViewportState::new(workspace.window.clone()),
                )
            })
            .collect();
        self.pending_detached_window_position_restore = self.detached_workspaces.keys().cloned().collect();
        self.pending_detached_reattach.clear();
        self.canvas_view = runtime_state.canvas_view_or_default();
        self.pan_target = None;
        self.initial_pan_done = runtime_state.has_persisted_canvas_view();
        self.startup_workspace_organization_pending = self.template_config.features.organize_workspaces_on_startup;
        self.runtime_dirty_since = None;
        self.git_watchers.clear();
        let needs_bootstrap = Self::runtime_state_needs_session_bootstrap(runtime_state);
        self.startup_bootstrap_failure = None;
        self.pending_startup_runtime_state = needs_bootstrap.then(|| runtime_state.clone());
        self.pending_startup_runtime_state_changed = false;
        self.startup_receiver = needs_bootstrap.then(|| Self::spawn_startup_bootstrap(runtime_state.clone()));
        if self.startup_receiver.is_some() {
            self.board = Board::new();
            self.board.attention_enabled = self.template_config.features.attention_feed;
        } else {
            self.restore_startup_runtime_state(runtime_state);
        }
    }

    pub(super) fn runtime_state_needs_session_bootstrap(runtime_state: &horizon_core::RuntimeState) -> bool {
        runtime_state.needs_agent_binding_bootstrap()
    }

    pub(super) fn spawn_startup_bootstrap(
        mut runtime_state: horizon_core::RuntimeState,
    ) -> Receiver<StartupBootstrapOutcome> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let exact_session_ids = runtime_state.exact_session_ids_requiring_validation();
            let needs_live_claude_sessions = runtime_state.needs_agent_binding_bootstrap_for(PanelKind::Claude);
            let bootstrap_catalog = match AgentSessionCatalog::load_for_runtime_state(&runtime_state) {
                Ok(catalog) => catalog,
                Err(error) => {
                    tracing::warn!("failed to validate saved agent sessions: {error}");
                    let _ = tx.send(StartupBootstrapOutcome::ExactValidationFailed(Box::new(
                        StartupBootstrapValidationFailure {
                            runtime_state,
                            message: error.to_string(),
                            unavailable_exact_session_ids: exact_session_ids,
                            all_exact_session_ids: true,
                            runtime_state_changed: false,
                        },
                    )));
                    return;
                }
            };
            let unavailable_exact_session_ids = bootstrap_catalog.unavailable_exact_session_ids().clone();
            let busy_session_ids = if needs_live_claude_sessions {
                live_claude_session_ids()
            } else {
                HashSet::new()
            };
            let runtime_state_changed =
                runtime_state.bootstrap_missing_agent_bindings(&bootstrap_catalog, &busy_session_ids);
            if !unavailable_exact_session_ids.is_empty() {
                let count = unavailable_exact_session_ids.len();
                let message = format!(
                    "{count} saved exact {} could not be verified.",
                    if count == 1 { "resume" } else { "resumes" }
                );
                let _ = tx.send(StartupBootstrapOutcome::ExactValidationFailed(Box::new(
                    StartupBootstrapValidationFailure {
                        runtime_state,
                        message,
                        unavailable_exact_session_ids,
                        all_exact_session_ids: false,
                        runtime_state_changed,
                    },
                )));
                return;
            }
            let _ = tx.send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
                runtime_state,
                session_catalog: bootstrap_catalog.into_catalog(),
                runtime_state_changed,
            })));
        });
        rx
    }

    fn spawn_session_catalog_refresh() -> Receiver<horizon_core::Result<AgentSessionCatalog>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(AgentSessionCatalog::load());
        });
        rx
    }

    pub(super) fn poll_startup_bootstrap(&mut self) -> bool {
        if self.startup_bootstrap_failure.is_some() {
            return false;
        }
        let Some(receiver) = self.startup_receiver.take() else {
            return true;
        };

        match receiver.try_recv() {
            Ok(StartupBootstrapOutcome::Ready(bootstrap)) => {
                self.session_catalog = bootstrap.session_catalog;
                self.last_session_catalog_refresh = Some(Instant::now());
                let runtime_state_changed =
                    self.pending_startup_runtime_state_changed || bootstrap.runtime_state_changed;
                if runtime_state_changed
                    && let Err(error) = self.save_recovered_startup_runtime_state(&bootstrap.runtime_state)
                {
                    self.pending_startup_runtime_state = Some(bootstrap.runtime_state);
                    self.pending_startup_runtime_state_changed = true;
                    self.startup_bootstrap_failure =
                        Some(StartupBootstrapFailure::RecoverySaveFailed { message: error });
                    return false;
                }
                self.restore_startup_runtime_state(&bootstrap.runtime_state);
                self.pending_startup_runtime_state = None;
                self.pending_startup_runtime_state_changed = false;
                true
            }
            Ok(StartupBootstrapOutcome::ExactValidationFailed(failure)) => {
                self.pending_startup_runtime_state = Some(failure.runtime_state);
                self.pending_startup_runtime_state_changed |= failure.runtime_state_changed;
                self.startup_bootstrap_failure = Some(StartupBootstrapFailure::ExactValidationFailed {
                    message: failure.message,
                    unavailable_exact_session_ids: failure.unavailable_exact_session_ids,
                    all_exact_session_ids: failure.all_exact_session_ids,
                });
                false
            }
            Err(TryRecvError::Empty) => {
                self.startup_receiver = Some(receiver);
                false
            }
            Err(TryRecvError::Disconnected) => {
                tracing::warn!("startup bootstrap worker disconnected before sending runtime state");
                self.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);
                false
            }
        }
    }

    pub(super) fn prepare_startup_bootstrap(&mut self, ctx: &egui::Context) -> bool {
        if self.poll_startup_bootstrap() {
            return true;
        }

        self.refresh_active_session_lease();
        if let Some(action) = render_loading_view(ctx, self.startup_bootstrap_failure.as_ref()) {
            self.handle_startup_bootstrap_failure(action);
            ctx.request_repaint();
            return false;
        }
        let repaint_after = if self.startup_bootstrap_failure.is_some() {
            STARTUP_BOOTSTRAP_FAILURE_REPAINT_INTERVAL
        } else {
            Duration::from_millis(16)
        };
        ctx.request_repaint_after(repaint_after);
        false
    }

    pub(super) fn handle_startup_bootstrap_failure(&mut self, action: StartupBootstrapFailureAction) {
        match action {
            StartupBootstrapFailureAction::Retry => {
                let Some(runtime_state) = self.pending_startup_runtime_state.clone() else {
                    return;
                };
                match self.startup_bootstrap_failure.as_ref() {
                    Some(
                        StartupBootstrapFailure::ExactValidationFailed { .. }
                        | StartupBootstrapFailure::WorkerDisconnected,
                    ) => {
                        self.startup_bootstrap_failure = None;
                        self.startup_receiver = Some(Self::spawn_startup_bootstrap(runtime_state));
                    }
                    Some(StartupBootstrapFailure::RecoverySaveFailed { .. }) => {
                        if let Err(error) = self.save_recovered_startup_runtime_state(&runtime_state) {
                            self.startup_bootstrap_failure =
                                Some(StartupBootstrapFailure::RecoverySaveFailed { message: error });
                            return;
                        }
                        self.finish_startup_recovery(&runtime_state);
                    }
                    None => {}
                }
            }
            StartupBootstrapFailureAction::ContinueWithoutExactResumes => {
                let Some(mut runtime_state) = self.pending_startup_runtime_state.clone() else {
                    return;
                };
                let unavailable_exact_session_ids = match self.startup_bootstrap_failure.as_ref() {
                    Some(StartupBootstrapFailure::ExactValidationFailed {
                        unavailable_exact_session_ids,
                        ..
                    }) => unavailable_exact_session_ids.clone(),
                    Some(StartupBootstrapFailure::WorkerDisconnected) => {
                        runtime_state.exact_session_ids_requiring_validation()
                    }
                    Some(StartupBootstrapFailure::RecoverySaveFailed { .. }) | None => return,
                };
                runtime_state.neutralize_unverified_session_bindings(&unavailable_exact_session_ids);
                let busy_claude_session_ids = live_claude_session_ids();
                runtime_state.normalize_agent_bindings(&busy_claude_session_ids);
                self.pending_startup_runtime_state = Some(runtime_state.clone());
                self.pending_startup_runtime_state_changed = true;
                if let Err(error) = self.save_recovered_startup_runtime_state(&runtime_state) {
                    self.startup_bootstrap_failure =
                        Some(StartupBootstrapFailure::RecoverySaveFailed { message: error });
                    return;
                }
                self.finish_startup_recovery(&runtime_state);
            }
            StartupBootstrapFailureAction::OpenWithoutSaving => {
                if !matches!(
                    self.startup_bootstrap_failure,
                    Some(StartupBootstrapFailure::RecoverySaveFailed { .. })
                ) {
                    return;
                }
                let Some(runtime_state) = self.pending_startup_runtime_state.clone() else {
                    return;
                };
                self.finish_startup_recovery(&runtime_state);
            }
        }
    }

    fn finish_startup_recovery(&mut self, runtime_state: &horizon_core::RuntimeState) {
        self.restore_startup_runtime_state(runtime_state);
        self.last_session_catalog_refresh = None;
        if self.session_catalog_refresh.is_none() {
            self.session_catalog_refresh = Some(Self::spawn_session_catalog_refresh());
        }
        self.pending_startup_runtime_state = None;
        self.pending_startup_runtime_state_changed = false;
        self.startup_bootstrap_failure = None;
        self.startup_receiver = None;
    }

    fn restore_startup_runtime_state(&mut self, runtime_state: &horizon_core::RuntimeState) {
        self.board = Board::from_runtime_state_with_transcripts(runtime_state, self.transcript_root.as_deref())
            .unwrap_or_else(|error| {
                tracing::error!("failed to restore runtime state: {error}");
                Board::new()
            });
        self.board.attention_enabled = self.template_config.features.attention_feed;
    }

    pub(super) fn refresh_active_session_lease(&mut self) {
        const LEASE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

        let Some(active_session) = self.active_session.as_mut() else {
            return;
        };
        if !active_session.persistent {
            return;
        }

        let Some(lease) = active_session.lease.as_mut() else {
            return;
        };
        if active_session
            .last_lease_refresh
            .is_some_and(|last_refresh| last_refresh.elapsed() < LEASE_REFRESH_INTERVAL)
        {
            return;
        }

        match self.session_store.refresh_lease(lease) {
            Ok(()) => active_session.last_lease_refresh = Some(Instant::now()),
            Err(error) => tracing::warn!("failed to refresh session lease: {error}"),
        }
    }

    pub(super) fn release_active_session_lease(&mut self) {
        let Some(active_session) = self.active_session.as_mut() else {
            return;
        };
        if !active_session.persistent {
            return;
        }

        if let Err(error) = self.session_store.release_lease(&active_session.session_id) {
            tracing::warn!("failed to release session lease: {error}");
        }
        active_session.lease = None;
        active_session.last_lease_refresh = None;
    }

    pub(super) fn maybe_refresh_session_catalog(&mut self) {
        const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

        if let Some(receiver) = self.session_catalog_refresh.take() {
            match receiver.try_recv() {
                Ok(Ok(catalog)) => {
                    self.session_catalog = catalog;
                    self.last_session_catalog_refresh = Some(Instant::now());
                    self.capture_new_agent_bindings();
                }
                Ok(Err(error)) => {
                    tracing::warn!("failed to refresh agent session catalog: {error}");
                    self.last_session_catalog_refresh = Some(Instant::now());
                }
                Err(TryRecvError::Empty) => {
                    self.session_catalog_refresh = Some(receiver);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("session catalog refresh worker disconnected");
                }
            }
        }

        let has_dynamic_agent = self.board.panels.iter().any(panel_uses_dynamic_binding);
        if !has_dynamic_agent {
            return;
        }

        let has_unbound_agent = self
            .board
            .panels
            .iter()
            .any(|panel| panel_uses_dynamic_binding(panel) && panel.session_binding.is_none());
        let has_recent_dynamic_output = self.board.panels.iter().any(|panel| {
            panel_uses_dynamic_binding(panel) && panel.had_recent_output_within(SESSION_BINDING_ACTIVITY_WINDOW)
        });
        if !has_unbound_agent && !has_recent_dynamic_output {
            return;
        }

        let should_refresh = self
            .last_session_catalog_refresh
            .is_none_or(|last_refresh| last_refresh.elapsed() >= REFRESH_INTERVAL);

        if should_refresh && self.session_catalog_refresh.is_none() {
            self.session_catalog_refresh = Some(Self::spawn_session_catalog_refresh());
        }
    }

    fn capture_new_agent_bindings(&mut self) {
        let reserved_session_keys: HashSet<AgentSessionKey> = self
            .board
            .panels
            .iter()
            .filter(|panel| matches!(panel.resume, PanelResume::Session { .. }))
            .filter_map(|panel| {
                panel
                    .session_binding
                    .as_ref()
                    .map(|binding| AgentSessionKey::new(panel.kind, &binding.session_id))
            })
            .collect();
        let dynamic_panels: Vec<_> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel_uses_dynamic_binding(panel))
            .map(|panel| DynamicPanelBindingState {
                panel_id: panel.id,
                kind: panel.kind,
                cwd: panel
                    .launch_cwd
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                launched_at_millis: panel.launched_at_millis,
                session_binding: panel.session_binding.clone(),
                recent_output: panel.had_recent_output_within(SESSION_BINDING_ACTIVITY_WINDOW),
            })
            .collect();
        let assignments = collect_dynamic_binding_updates(&dynamic_panels, &reserved_session_keys, |kind, cwd| {
            self.session_catalog.recent_for(kind, cwd)
        });

        if assignments.is_empty() {
            return;
        }

        for (panel_id, binding) in assignments {
            if let Some(panel) = self.board.panel_mut(panel_id) {
                panel.set_session_binding(Some(binding));
            }
        }
        self.mark_runtime_dirty();
    }

    pub(super) fn session_rebind_options(&self, panel_id: PanelId) -> Vec<(String, AgentSessionBinding)> {
        let Some(panel) = self.board.panel(panel_id) else {
            return Vec::new();
        };
        if !panel.kind.supports_session_binding() {
            return Vec::new();
        }

        let cwd = panel.launch_cwd.as_ref().map(|path| path.display().to_string());
        let current_session_id = panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str());
        let reserved_session_ids: HashSet<_> = self
            .board
            .panels
            .iter()
            .filter(|candidate| candidate.id != panel_id && candidate.kind == panel.kind)
            .filter_map(panel_session_id)
            .collect();
        self.session_catalog
            .recent_for(panel.kind, cwd.as_deref())
            .into_iter()
            .filter(|session| {
                Some(session.session_id.as_str()) != current_session_id
                    && !reserved_session_ids.contains(session.session_id.as_str())
            })
            .take(8)
            .map(|session| {
                let short_id = short_session_id(&session.session_id);
                let label = truncate_session_label(
                    &session
                        .label
                        .clone()
                        .unwrap_or_else(|| format!("{} session", panel.kind.display_name())),
                );
                (format!("{label} · {short_id}"), session.into_binding())
            })
            .collect()
    }

    pub(super) fn rebind_and_restart_panel_session(&mut self, panel_id: PanelId, binding: AgentSessionBinding) -> bool {
        let Some(panel) = self.board.panel(panel_id) else {
            return false;
        };
        if panel.kind != binding.kind
            || self.board.panels.iter().any(|candidate| {
                candidate.id != panel_id
                    && candidate.kind == binding.kind
                    && panel_session_id(candidate) == Some(binding.session_id.as_str())
            })
        {
            return false;
        }
        let Some(panel) = self.board.panel_mut(panel_id) else {
            return false;
        };

        panel.resume = PanelResume::Session {
            session_id: binding.session_id.clone(),
        };
        panel.set_session_binding(Some(binding));
        self.queue_panel_restart(panel_id);
        true
    }
}

#[cfg(test)]
mod tests;
