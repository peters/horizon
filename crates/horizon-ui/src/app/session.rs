use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{AgentSessionBinding, AgentSessionCatalog, Board, PanelId, PanelKind, PanelOptions, PanelResume};

use crate::{loading_spinner, theme};

use super::util::{empty_string_as_none, short_session_id, truncate_session_label};
use super::{ActiveSession, HorizonApp, ResolvedSession, StartupBootstrap};

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
            .map(|workspace| (workspace.workspace_local_id.clone(), workspace.window.clone()))
            .collect();
        self.pending_detached_window_position_restore = self.detached_workspaces.keys().cloned().collect();
        self.pending_detached_reattach.clear();
        self.canvas_view = runtime_state.canvas_view_or_default();
        self.pan_target = None;
        self.initial_pan_done = runtime_state.has_persisted_canvas_view();
        self.runtime_dirty_since = None;
        self.git_watchers.clear();
        self.startup_receiver = Self::runtime_state_needs_session_bootstrap(runtime_state)
            .then(|| Self::spawn_startup_bootstrap(runtime_state.clone()));
        self.board = if self.startup_receiver.is_some() {
            Board::new()
        } else {
            Board::from_runtime_state_with_transcripts(runtime_state, self.transcript_root.as_deref()).unwrap_or_else(
                |error| {
                    tracing::error!("failed to restore runtime state: {error}");
                    Board::new()
                },
            )
        };
        self.board.attention_enabled = self.template_config.features.attention_feed;
    }

    pub(super) fn runtime_state_needs_session_bootstrap(runtime_state: &horizon_core::RuntimeState) -> bool {
        runtime_state
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.panels)
            .any(|panel| {
                panel.kind.is_agent() && panel.session_binding.is_none() && matches!(panel.resume, PanelResume::Last)
            })
    }

    fn panel_options_need_session_bootstrap(opts: &PanelOptions) -> bool {
        opts.kind.is_agent() && opts.session_binding.is_none() && matches!(opts.resume, PanelResume::Last)
    }

    pub(super) fn spawn_startup_bootstrap(mut runtime_state: horizon_core::RuntimeState) -> Receiver<StartupBootstrap> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let session_catalog = AgentSessionCatalog::load().unwrap_or_else(|error| {
                tracing::warn!("failed to load agent session catalog: {error}");
                AgentSessionCatalog::default()
            });
            runtime_state.bootstrap_missing_agent_bindings(&session_catalog);
            let _ = tx.send(StartupBootstrap {
                runtime_state,
                session_catalog,
            });
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

    pub(super) fn resolve_panel_launch_binding(&mut self, opts: &mut PanelOptions) {
        if !Self::panel_options_need_session_bootstrap(opts) {
            return;
        }

        match AgentSessionCatalog::load() {
            Ok(catalog) => {
                let cwd = opts.cwd.as_ref().map(|path| path.display().to_string());
                opts.session_binding = catalog
                    .recent_for(opts.kind, cwd.as_deref())
                    .into_iter()
                    .next()
                    .map(horizon_core::AgentSessionRecord::into_binding);
                self.session_catalog = catalog;
                self.last_session_catalog_refresh = Some(Instant::now());
            }
            Err(error) => tracing::warn!("failed to load agent session catalog: {error}"),
        }
    }

    pub(super) fn poll_startup_bootstrap(&mut self) -> bool {
        let Some(receiver) = self.startup_receiver.take() else {
            return true;
        };

        match receiver.try_recv() {
            Ok(bootstrap) => {
                self.session_catalog = bootstrap.session_catalog;
                self.last_session_catalog_refresh = Some(Instant::now());
                self.board = Board::from_runtime_state_with_transcripts(
                    &bootstrap.runtime_state,
                    self.transcript_root.as_deref(),
                )
                .unwrap_or_else(|error| {
                    tracing::error!("failed to restore runtime state: {error}");
                    Board::new()
                });
                self.board.attention_enabled = self.template_config.features.attention_feed;
                true
            }
            Err(TryRecvError::Empty) => {
                self.startup_receiver = Some(receiver);
                false
            }
            Err(TryRecvError::Disconnected) => {
                tracing::warn!("startup bootstrap worker disconnected before sending runtime state");
                true
            }
        }
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

        let has_unbound_agent = self
            .board
            .panels
            .iter()
            .any(|panel| panel.kind.is_agent() && panel.session_binding.is_none());
        if !has_unbound_agent {
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
        let mut used_session_ids: HashSet<String> = self
            .board
            .panels
            .iter()
            .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone()))
            .collect();
        let mut pending_panels: HashMap<(PanelKind, String), Vec<(PanelId, i64)>> = HashMap::new();

        for panel in &self.board.panels {
            if panel.kind.is_agent() && panel.session_binding.is_none() {
                let cwd = panel
                    .launch_cwd
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                pending_panels
                    .entry((panel.kind, cwd))
                    .or_default()
                    .push((panel.id, panel.launched_at_millis));
            }
        }

        let mut assignments = Vec::new();
        for ((kind, cwd), mut panels) in pending_panels {
            panels.sort_by(|left, right| right.1.cmp(&left.1));
            let oldest_launch = panels.iter().map(|(_, launched_at)| *launched_at).min().unwrap_or(0);
            let mut candidates = self.session_catalog.recent_for(kind, empty_string_as_none(&cwd));
            candidates.retain(|candidate| {
                !used_session_ids.contains(&candidate.session_id)
                    && candidate.updated_at >= oldest_launch.saturating_sub(300_000)
            });
            candidates.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));

            for ((panel_id, _), candidate) in panels.into_iter().zip(candidates) {
                used_session_ids.insert(candidate.session_id.clone());
                assignments.push((panel_id, candidate.into_binding()));
            }
        }

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
        if !panel.kind.is_agent() {
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

    pub(super) fn apply_pending_session_rebinds(&mut self) {
        if self.pending_session_rebinds.is_empty() {
            return;
        }

        let mut changed = false;
        for (panel_id, binding) in self.pending_session_rebinds.drain(..) {
            if let Some(panel) = self.board.panel_mut(panel_id) {
                panel.set_session_binding(Some(binding));
                changed = true;
            }
        }

        if changed {
            self.mark_runtime_dirty();
        }
    }
}

pub(super) fn render_loading_view(ctx: &Context) {
    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::BG))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.28);
                ui.label(egui::RichText::new("Horizon").size(26.0).strong().color(theme::FG));
                ui.add_space(16.0);
            });
            loading_spinner::show(
                ui,
                egui::Id::new("startup_loading_spinner"),
                Some("Resolving saved sessions\u{2026}"),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::HorizonApp;
    use horizon_core::{PanelKind, PanelResume, PanelState, RuntimeState, WorkspaceState};

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
    fn runtime_state_skips_bootstrap_for_fresh_or_bound_panels() {
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
                        resume: PanelResume::Last,
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

        assert!(!HorizonApp::runtime_state_needs_session_bootstrap(&state));
    }
}
