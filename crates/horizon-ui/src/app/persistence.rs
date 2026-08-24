use std::time::{Duration, Instant};

use horizon_core::{DetachedWorkspaceState, RuntimeState};

use super::HorizonApp;

impl HorizonApp {
    pub(super) fn save_recovered_startup_runtime_state(&self, runtime_state: &RuntimeState) -> Result<(), String> {
        let Some(active_session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            return Ok(());
        };
        self.session_store
            .save_runtime_state(&active_session.session_id, runtime_state)
            .map_err(|error| error.to_string())
    }

    pub(super) fn mark_runtime_dirty(&mut self) {
        self.runtime_dirty_since.get_or_insert_with(Instant::now);
    }

    pub(super) fn flush_runtime_if_dirty(&mut self) {
        const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
        if let Some(since) = self.runtime_dirty_since
            && since.elapsed() >= SAVE_DEBOUNCE
        {
            if self.auto_save_runtime_state() {
                self.runtime_dirty_since = None;
            } else {
                self.runtime_dirty_since = Some(Instant::now());
            }
        }
    }

    #[must_use]
    pub(super) fn auto_save_runtime_state(&self) -> bool {
        let Some(active_session) = self.active_session.as_ref().filter(|session| session.persistent) else {
            return true;
        };

        if self.pending_startup_runtime_state.is_some() || self.root_viewport_stabilization_blocks_interaction() {
            tracing::debug!("preserving the prior runtime snapshot while session view initialization is pending");
            return false;
        }

        let detached_workspaces = self
            .detached_workspaces
            .iter()
            .filter(|(workspace_local_id, _)| !self.pending_detached_reattach.contains(*workspace_local_id))
            .map(|(workspace_local_id, state)| DetachedWorkspaceState {
                workspace_local_id: workspace_local_id.clone(),
                window: state.window.clone(),
            })
            .collect();

        let mut runtime_state = RuntimeState::from_board_with_detached_workspaces(
            &self.board,
            self.window_config.clone(),
            self.canvas_view,
            detached_workspaces,
        );
        runtime_state.browser = self.template_config.browser.clone();
        if let Err(error) = self
            .session_store
            .save_runtime_state(&active_session.session_id, &runtime_state)
        {
            tracing::error!("failed to auto-save runtime state: {error}");
            return false;
        }
        true
    }

    pub(super) fn sync_window_config(&mut self, ctx: &egui::Context) {
        if self.root_viewport_stabilizer.is_some() {
            return;
        }

        ctx.input(|input| {
            if let Some(rect) = input.viewport().inner_rect {
                let new_w = rect.width();
                let new_h = rect.height();
                if (new_w - self.window_config.width).abs() > 1.0 || (new_h - self.window_config.height).abs() > 1.0 {
                    self.window_config.width = new_w;
                    self.window_config.height = new_h;
                    self.mark_runtime_dirty();
                }
            }
            if let Some(pos) = input.viewport().outer_rect {
                let new_x = pos.min.x;
                let new_y = pos.min.y;
                let changed = self.window_config.x.is_none_or(|x| (x - new_x).abs() > 1.0)
                    || self.window_config.y.is_none_or(|y| (y - new_y).abs() > 1.0);
                if changed {
                    self.window_config.x = Some(new_x);
                    self.window_config.y = Some(new_y);
                    self.mark_runtime_dirty();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::{Config, PanelKind, PanelState, RuntimeState, StartupDecision, WorkspaceState};

    use super::super::test_support::test_app_with_config_and_startup;

    #[test]
    fn auto_save_persists_the_active_browser_profile_configuration() {
        let profile_root = tempfile::tempdir().expect("profile temp dir");
        let launched_root = profile_root.path().join("launched-profiles");
        let current_root = profile_root.path().join("current-profiles");
        let mut launched_config = Config::default();
        launched_config.browser.command = Some(profile_root.path().join("missing-chrome").display().to_string());
        launched_config.browser.profile_root = Some(launched_root.clone());
        let startup_runtime = RuntimeState {
            browser: launched_config.browser.clone(),
            workspaces: vec![WorkspaceState {
                local_id: "browser-workspace".to_string(),
                panels: vec![PanelState {
                    local_id: "browser-panel".to_string(),
                    name: "Browser".to_string(),
                    kind: PanelKind::Browser,
                    ..PanelState::default()
                }],
                ..WorkspaceState::default()
            }],
            ..RuntimeState::default()
        };
        let (_temp, _ctx, mut app) = test_app_with_config_and_startup(
            &launched_config,
            StartupDecision::Ephemeral {
                runtime_state: Box::new(startup_runtime.clone()),
            },
        );
        let session = app
            .session_store
            .create_session_from_runtime(startup_runtime)
            .expect("create persistent session");
        app.activate_persistent_session(&session);
        let mut current_config = launched_config.clone();
        current_config.browser.profile_root = Some(current_root);
        current_config.browser.quality = 73;
        app.apply_runtime_config(&current_config);

        assert!(app.auto_save_runtime_state());

        let saved = RuntimeState::load(&session.runtime_state_path)
            .expect("load saved runtime")
            .expect("saved runtime exists");
        assert_eq!(saved.browser, current_config.browser);
        assert_eq!(
            saved.workspaces[0].panels[0]
                .browser_profile
                .as_ref()
                .and_then(|profile| profile.root.as_ref()),
            Some(&launched_root)
        );
    }
}
