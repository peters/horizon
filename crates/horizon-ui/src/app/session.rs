use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, Board, PanelId, PanelKind, PanelResume, live_claude_session_ids,
};

use super::util::{empty_string_as_none, short_session_id, truncate_session_label};
use super::{
    ActiveSession, DetachedWorkspaceViewportState, HorizonApp, ResolvedSession, StartupBootstrap,
    StartupBootstrapFailure, StartupBootstrapOutcome,
};

const SESSION_BINDING_ACTIVITY_WINDOW: Duration = Duration::from_secs(10);

mod loading;

pub(super) use loading::render_loading_view;

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
}

fn collect_dynamic_binding_updates(
    dynamic_panels: &[DynamicPanelBindingState],
    reserved_session_ids: &HashSet<String>,
    recent_for: impl Fn(PanelKind, Option<&str>) -> Vec<horizon_core::AgentSessionRecord>,
) -> Vec<(PanelId, AgentSessionBinding)> {
    let mut used_session_ids = reserved_session_ids.clone();
    used_session_ids.extend(
        dynamic_panels
            .iter()
            .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone())),
    );

    let mut grouped_panels: HashMap<(PanelKind, String), Vec<&DynamicPanelBindingState>> = HashMap::new();
    for panel in dynamic_panels {
        grouped_panels
            .entry((panel.kind, panel.cwd.clone()))
            .or_default()
            .push(panel);
    }

    let mut assignments = Vec::new();
    for ((kind, cwd), panels) in grouped_panels {
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
                    && !used_session_ids.contains(&candidate.session_id)
            }) {
                used_session_ids.insert(candidate.session_id.clone());
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
                !used_session_ids.contains(&candidate.session_id)
                    && candidate.updated_at >= oldest_launch.saturating_sub(300_000)
            })
            .collect();
        for (panel, candidate) in unbound_panels.into_iter().zip(candidates) {
            used_session_ids.insert(candidate.session_id.clone());
            assignments.push((panel.panel_id, candidate.into_binding()));
        }
    }

    assignments
}

fn panel_uses_dynamic_binding(panel: &horizon_core::Panel) -> bool {
    panel.kind.supports_session_binding()
        && !matches!(panel.resume, PanelResume::Session { .. })
        && panel.session_binding.as_ref().is_none_or(|binding| binding.resumable)
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
        self.runtime_dirty_since = None;
        self.git_watchers.clear();
        let needs_bootstrap = Self::runtime_state_needs_session_bootstrap(runtime_state);
        self.startup_bootstrap_failure = None;
        self.pending_startup_runtime_state = needs_bootstrap.then(|| runtime_state.clone());
        self.startup_receiver = needs_bootstrap.then(|| Self::spawn_startup_bootstrap(runtime_state.clone()));
        if self.startup_receiver.is_some() {
            self.board = Board::new();
            self.board.attention_enabled = self.template_config.features.attention_feed;
        } else {
            self.restore_startup_runtime_state(runtime_state);
        }
    }

    pub(super) fn runtime_state_needs_session_bootstrap(runtime_state: &horizon_core::RuntimeState) -> bool {
        runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .any(|panel| {
                (panel.kind.requires_exact_session_validation() && panel.stored_session_id().is_some())
                    || (panel.kind.supports_session_binding()
                        && panel.session_binding.is_none()
                        && matches!(panel.resume, PanelResume::Last))
            })
    }

    pub(super) fn spawn_startup_bootstrap(
        mut runtime_state: horizon_core::RuntimeState,
    ) -> Receiver<StartupBootstrapOutcome> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let bootstrap_catalog = AgentSessionCatalog::load_for_runtime_state(&runtime_state);
            let busy_session_ids = live_claude_session_ids();
            let runtime_state_changed =
                runtime_state.bootstrap_missing_agent_bindings(&bootstrap_catalog, &busy_session_ids);
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
                self.pending_startup_runtime_state = None;
                self.session_catalog = bootstrap.session_catalog;
                self.last_session_catalog_refresh = Some(Instant::now());
                self.restore_startup_runtime_state(&bootstrap.runtime_state);
                if bootstrap.runtime_state_changed {
                    self.mark_runtime_dirty();
                }
                true
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

    pub(super) fn handle_startup_bootstrap_failure(&mut self, action: StartupBootstrapFailureAction) {
        match action {
            StartupBootstrapFailureAction::Retry => {
                if !matches!(
                    self.startup_bootstrap_failure,
                    Some(StartupBootstrapFailure::WorkerDisconnected)
                ) {
                    return;
                }
                let Some(runtime_state) = self.pending_startup_runtime_state.clone() else {
                    return;
                };
                self.startup_bootstrap_failure = None;
                self.startup_receiver = Some(Self::spawn_startup_bootstrap(runtime_state));
            }
            StartupBootstrapFailureAction::ContinueWithoutExactResumes => {
                if !matches!(
                    self.startup_bootstrap_failure,
                    Some(StartupBootstrapFailure::WorkerDisconnected | StartupBootstrapFailure::RecoverySaveFailed(_))
                ) {
                    return;
                }
                let Some(mut runtime_state) = self.pending_startup_runtime_state.clone() else {
                    return;
                };
                runtime_state.retain_unverified_session_bindings();
                if let Err(error) = self.save_recovered_startup_runtime_state(&runtime_state) {
                    self.startup_bootstrap_failure = Some(StartupBootstrapFailure::RecoverySaveFailed(error));
                    return;
                }
                self.restore_startup_runtime_state(&runtime_state);
                self.pending_startup_runtime_state = None;
                self.startup_bootstrap_failure = None;
                self.startup_receiver = None;
            }
        }
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
        let reserved_session_ids: HashSet<String> = self
            .board
            .panels
            .iter()
            .filter(|panel| matches!(panel.resume, PanelResume::Session { .. }))
            .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone()))
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
        let assignments = collect_dynamic_binding_updates(&dynamic_panels, &reserved_session_ids, |kind, cwd| {
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
        self.session_catalog
            .recent_for(panel.kind, cwd.as_deref())
            .into_iter()
            .filter(|session| Some(session.session_id.as_str()) != current_session_id)
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
mod tests {
    use std::collections::HashSet;

    use super::{
        DynamicPanelBindingState, HorizonApp, StartupBootstrapFailureAction, collect_dynamic_binding_updates,
        panel_uses_dynamic_binding,
    };
    use egui::Context;
    use horizon_core::{
        AgentSessionBinding, Config, HorizonHome, PanelId, PanelKind, PanelOptions, PanelResume, PanelState,
        RuntimeState, SessionStore, StartupDecision, WorkspaceState,
    };
    use tempfile::TempDir;

    use crate::app::StartupBootstrapFailure;
    use crate::input;

    fn test_app() -> (TempDir, HorizonApp) {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.yaml");
        let session_store = SessionStore::new(
            HorizonHome::from_root(temp.path().join(".horizon")),
            config_path.clone(),
        );
        let config = Config::default();
        let ctx = Context::default();
        let app = HorizonApp::new_with_egui_context(
            &ctx,
            &config,
            config_path,
            session_store,
            StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
            input::ObservedKeyboardInputs::default(),
        );
        (temp, app)
    }

    fn test_persistent_recovery_app(runtime_state: RuntimeState) -> (TempDir, HorizonApp, std::path::PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.yaml");
        let session_store = SessionStore::new(
            HorizonHome::from_root(temp.path().join(".horizon")),
            config_path.clone(),
        );
        let pending_runtime_state = runtime_state.clone();
        let mut session = session_store
            .create_session_from_runtime(runtime_state)
            .expect("create persistent session");
        let runtime_path = session.runtime_state_path.clone();
        // Construct the app without starting the real provider-catalog worker;
        // these tests inject the failed pre-open state below.
        session.runtime_state = RuntimeState::default();
        let ctx = Context::default();
        let mut app = HorizonApp::new_with_egui_context(
            &ctx,
            &Config::default(),
            config_path,
            session_store,
            StartupDecision::Open {
                disposition: horizon_core::SessionOpenDisposition::Resume,
                session: Box::new(session),
            },
            input::ObservedKeyboardInputs::default(),
        );
        assert!(app.startup_receiver.is_none());
        app.pending_startup_runtime_state = Some(pending_runtime_state);
        (temp, app, runtime_path)
    }

    #[cfg(windows)]
    fn exiting_command() -> (String, Vec<String>) {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "exit".to_string(), "0".to_string()],
        )
    }

    #[cfg(not(windows))]
    fn exiting_command() -> (String, Vec<String>) {
        ("/bin/sh".to_string(), vec!["-c".to_string(), "exit 0".to_string()])
    }

    #[test]
    fn runtime_state_needs_bootstrap_for_unbound_last_agent_panel() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                cwd: None,
                position: None,
                template: None,
                layout: None,
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Claude".to_string(),
                    kind: PanelKind::Claude,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                }],
            }],
            ..RuntimeState::default()
        };

        assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn runtime_state_needs_bootstrap_for_unbound_last_opencode_panel() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                cwd: None,
                position: None,
                template: None,
                layout: None,
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "OpenCode".to_string(),
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                }],
            }],
            ..RuntimeState::default()
        };

        assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn runtime_state_needs_bootstrap_for_unbound_last_pi_panel() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                cwd: None,
                position: None,
                template: None,
                layout: None,
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Pi".to_string(),
                    kind: PanelKind::Pi,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                }],
            }],
            ..RuntimeState::default()
        };

        assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn runtime_state_needs_bootstrap_for_a_persisted_codex_binding() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                cwd: None,
                position: None,
                template: None,
                layout: None,
                panels: vec![
                    PanelState {
                        local_id: "fresh".to_string(),
                        name: "Shell".to_string(),
                        kind: PanelKind::Shell,
                        resume: PanelResume::Fresh,
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "bound".to_string(),
                        name: "Codex".to_string(),
                        kind: PanelKind::Codex,
                        resume: PanelResume::Fresh,
                        session_binding: Some(horizon_core::AgentSessionBinding::new(
                            PanelKind::Codex,
                            "session-9".to_string(),
                            None,
                            None,
                            None,
                        )),
                        ..PanelState::default()
                    },
                ],
            }],
            ..RuntimeState::default()
        };

        assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn runtime_state_needs_bootstrap_for_an_explicit_codex_session() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Session {
                        session_id: "session-9".to_string(),
                    },
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn runtime_state_skips_bootstrap_for_a_bound_non_codex_panel() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "OpenCode".to_string(),
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Last,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::OpenCode,
                        "session-9".to_string(),
                        None,
                        None,
                        None,
                    )),
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };

        assert!(!HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn disconnected_bootstrap_enters_a_stable_failed_state() {
        let (_temp, mut app) = test_app();
        let (tx, rx) = std::sync::mpsc::channel();
        app.startup_receiver = Some(rx);
        app.pending_startup_runtime_state = Some(RuntimeState::default());
        drop(tx);

        assert!(!app.poll_startup_bootstrap());
        assert!(matches!(
            app.startup_bootstrap_failure,
            Some(StartupBootstrapFailure::WorkerDisconnected)
        ));
        assert!(app.startup_receiver.is_none());
        assert!(app.pending_startup_runtime_state.is_some());
        assert!(!app.poll_startup_bootstrap());
    }

    #[test]
    fn failed_bootstrap_can_continue_without_saved_codex_resumes() {
        let (_temp, mut app) = test_app();
        let (command, args) = exiting_command();
        app.pending_startup_runtime_state = Some(RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        });
        app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

        app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

        assert!(app.startup_bootstrap_failure.is_none());
        assert!(app.startup_receiver.is_none());
        assert!(app.pending_startup_runtime_state.is_none());
        let panel = &app.board.panels[0];
        assert_eq!(
            panel
                .session_binding
                .as_ref()
                .map(|binding| binding.session_id.as_str()),
            Some("session-child")
        );
        assert!(panel.session_binding.as_ref().is_some_and(|binding| !binding.resumable));
        assert!(matches!(panel.resume, PanelResume::Fresh));
    }

    #[test]
    fn persistent_recovery_is_saved_before_the_board_opens() {
        let (command, args) = exiting_command();
        let runtime_state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };
        let (_temp, mut app, runtime_path) = test_persistent_recovery_app(runtime_state);
        app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

        app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

        let saved = RuntimeState::load(&runtime_path)
            .expect("load repaired runtime")
            .expect("repaired runtime exists");
        assert!(saved.workspaces[0].panels[0].exact_session_id().is_none());
        assert_eq!(saved.workspaces[0].panels[0].stored_session_id(), Some("session-child"));
        assert!(app.pending_startup_runtime_state.is_none());
        assert!(app.startup_bootstrap_failure.is_none());
        assert!(
            app.board.panels[0]
                .session_binding
                .as_ref()
                .is_some_and(|binding| !binding.resumable)
        );
    }

    #[test]
    fn failed_recovery_save_keeps_pending_state_until_retry_succeeds() {
        let (command, args) = exiting_command();
        let runtime_state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                panels: vec![PanelState {
                    local_id: "panel".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    resume: PanelResume::Session {
                        session_id: "session-child".to_string(),
                    },
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };
        let (temp, mut app, runtime_path) = test_persistent_recovery_app(runtime_state);
        let blocked_home = temp.path().join("blocked-home");
        std::fs::write(&blocked_home, "not a directory").expect("create blocked home path");
        app.session_store = SessionStore::new(HorizonHome::from_root(blocked_home), temp.path().join("config.yaml"));
        app.startup_receiver = None;
        app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

        app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

        assert!(matches!(
            app.startup_bootstrap_failure,
            Some(StartupBootstrapFailure::RecoverySaveFailed(_))
        ));
        assert!(app.pending_startup_runtime_state.is_some());
        assert!(app.board.panels.is_empty());

        app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::Retry);

        assert!(matches!(
            app.startup_bootstrap_failure,
            Some(StartupBootstrapFailure::RecoverySaveFailed(_))
        ));
        assert!(app.startup_receiver.is_none());
        assert!(app.pending_startup_runtime_state.is_some());
        assert!(app.board.panels.is_empty());

        app.session_store = SessionStore::new(
            HorizonHome::from_root(temp.path().join(".horizon")),
            temp.path().join("config.yaml"),
        );
        app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

        let saved = RuntimeState::load(&runtime_path)
            .expect("load repaired runtime")
            .expect("repaired runtime exists");
        assert!(saved.workspaces[0].panels[0].exact_session_id().is_none());
        assert_eq!(saved.workspaces[0].panels[0].stored_session_id(), Some("session-child"));
        assert!(matches!(saved.workspaces[0].panels[0].resume, PanelResume::Fresh));
        assert!(app.pending_startup_runtime_state.is_none());
        assert!(app.startup_bootstrap_failure.is_none());
        assert!(
            app.board.panels[0]
                .session_binding
                .as_ref()
                .is_some_and(|binding| !binding.resumable)
        );
    }

    #[test]
    fn runtime_state_skips_bootstrap_for_agents_without_exact_session_catalogs() {
        let state = RuntimeState {
            workspaces: vec![WorkspaceState {
                local_id: "workspace".to_string(),
                name: "alpha".to_string(),
                cwd: None,
                position: None,
                template: None,
                layout: None,
                panels: vec![
                    PanelState {
                        local_id: "gemini".to_string(),
                        name: "Gemini".to_string(),
                        kind: PanelKind::Gemini,
                        resume: PanelResume::Last,
                        ..PanelState::default()
                    },
                    PanelState {
                        local_id: "kilo".to_string(),
                        name: "KiloCode".to_string(),
                        kind: PanelKind::KiloCode,
                        resume: PanelResume::Last,
                        ..PanelState::default()
                    },
                ],
            }],
            ..RuntimeState::default()
        };

        assert!(!HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }

    #[test]
    fn collect_dynamic_binding_updates_assigns_unbound_panels() {
        let panels = vec![DynamicPanelBindingState {
            panel_id: PanelId(7),
            kind: PanelKind::Codex,
            cwd: "/repo".to_string(),
            launched_at_millis: 10,
            session_binding: None,
            recent_output: false,
        }];
        let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
            assert_eq!(kind, PanelKind::Codex);
            assert_eq!(cwd, Some("/repo"));
            vec![horizon_core::AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: "session-1".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 12,
                interactive: true,
            }]
        });

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, PanelId(7));
        assert_eq!(updates[0].1.session_id, "session-1");
    }

    #[test]
    fn collect_dynamic_binding_updates_refreshes_single_recently_active_panel() {
        let panels = vec![DynamicPanelBindingState {
            panel_id: PanelId(7),
            kind: PanelKind::Codex,
            cwd: "/repo".to_string(),
            launched_at_millis: 10,
            session_binding: Some(horizon_core::AgentSessionBinding::new(
                PanelKind::Codex,
                "session-old".to_string(),
                Some("/repo".to_string()),
                None,
                Some(12),
            )),
            recent_output: true,
        }];
        let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
            assert_eq!(kind, PanelKind::Codex);
            assert_eq!(cwd, Some("/repo"));
            vec![
                horizon_core::AgentSessionRecord {
                    kind: PanelKind::Codex,
                    session_id: "session-new".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 20,
                    interactive: true,
                },
                horizon_core::AgentSessionRecord {
                    kind: PanelKind::Codex,
                    session_id: "session-old".to_string(),
                    cwd: Some("/repo".to_string()),
                    label: None,
                    updated_at: 12,
                    interactive: true,
                },
            ]
        });

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, PanelId(7));
        assert_eq!(updates[0].1.session_id, "session-new");
    }

    #[test]
    fn collect_dynamic_binding_updates_does_not_reassign_ambiguous_recent_group() {
        let panels = vec![
            DynamicPanelBindingState {
                panel_id: PanelId(7),
                kind: PanelKind::Codex,
                cwd: "/repo".to_string(),
                launched_at_millis: 10,
                session_binding: Some(horizon_core::AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-a".to_string(),
                    Some("/repo".to_string()),
                    None,
                    Some(12),
                )),
                recent_output: true,
            },
            DynamicPanelBindingState {
                panel_id: PanelId(8),
                kind: PanelKind::Codex,
                cwd: "/repo".to_string(),
                launched_at_millis: 11,
                session_binding: Some(horizon_core::AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-b".to_string(),
                    Some("/repo".to_string()),
                    None,
                    Some(13),
                )),
                recent_output: true,
            },
        ];
        let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
            assert_eq!(kind, PanelKind::Codex);
            assert_eq!(cwd, Some("/repo"));
            vec![horizon_core::AgentSessionRecord {
                kind: PanelKind::Codex,
                session_id: "session-c".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 20,
                interactive: true,
            }]
        });

        assert!(updates.is_empty());
    }

    #[test]
    fn collect_dynamic_binding_updates_does_not_reassign_claude_bindings() {
        let panels = vec![DynamicPanelBindingState {
            panel_id: PanelId(7),
            kind: PanelKind::Claude,
            cwd: "/repo".to_string(),
            launched_at_millis: 10,
            session_binding: Some(horizon_core::AgentSessionBinding::new(
                PanelKind::Claude,
                "preassigned-session".to_string(),
                Some("/repo".to_string()),
                None,
                Some(12),
            )),
            recent_output: true,
        }];
        let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
            assert_eq!(kind, PanelKind::Claude);
            assert_eq!(cwd, Some("/repo"));
            vec![horizon_core::AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "external-newer-session".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 20,
                interactive: true,
            }]
        });

        assert!(updates.is_empty());
    }

    #[test]
    fn retained_session_ids_are_not_replaced_by_dynamic_catalog_matching() {
        let (_temp, mut app) = test_app();
        let workspace_id = app.board.create_workspace("test");
        let (command, args) = exiting_command();
        let panel_id = app
            .board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    resume: PanelResume::Last,
                    session_binding: Some(
                        AgentSessionBinding::new(PanelKind::Codex, "saved-child".to_string(), None, None, None)
                            .retained_unresumable(),
                    ),
                    ..PanelOptions::default()
                },
                workspace_id,
            )
            .expect("create retained panel");

        assert!(!panel_uses_dynamic_binding(
            app.board.panel(panel_id).expect("retained panel")
        ));
    }

    #[test]
    fn rebind_and_restart_updates_the_binding_and_queues_the_panel() {
        let (_temp, mut app) = test_app();
        let workspace_id = app.board.create_workspace("test");
        let (command, args) = exiting_command();
        let panel_id = app
            .board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Codex,
                    command: Some(command),
                    args,
                    ..PanelOptions::default()
                },
                workspace_id,
            )
            .expect("create agent panel");
        let binding = AgentSessionBinding::new(
            PanelKind::Codex,
            "session-2".to_string(),
            Some("/repo".to_string()),
            Some("Recovered session".to_string()),
            Some(42),
        );

        assert!(app.rebind_and_restart_panel_session(panel_id, binding.clone()));
        assert!(app.rebind_and_restart_panel_session(panel_id, binding.clone()));

        let panel = app.board.panel(panel_id).expect("rebound panel");
        assert_eq!(
            panel.resume,
            PanelResume::Session {
                session_id: "session-2".to_string(),
            }
        );
        assert_eq!(panel.session_binding.as_ref(), Some(&binding));
        assert_eq!(app.panels_to_restart, vec![panel_id]);
    }
}
