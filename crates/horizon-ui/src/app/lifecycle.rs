use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Config, GitWatcher, PanelKind, WorkspaceId};

use crate::{loading_spinner, theme};

use super::{HorizonApp, WS_BG_PAD, WS_TITLE_HEIGHT, attention_feed};

impl HorizonApp {
    #[profiling::function]
    pub(super) fn exit_on_close_request(&mut self, ctx: &Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }

        // Keep the viewport alive while we flush state and stop PTY-backed panels.
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.begin_shutdown();
    }

    /// Starts asynchronous terminal shutdown. State is saved immediately,
    /// and background threads join each terminal event loop. The UI shows a
    /// progress overlay until all terminals are done or the budget expires.
    #[profiling::function]
    fn begin_shutdown(&mut self) {
        if self.shutdown_progress.is_some() {
            return;
        }

        self.auto_save_runtime_state();
        self.git_watchers.clear();
        self.shutdown_progress = Some(self.board.begin_async_shutdown());
    }

    #[profiling::function]
    pub(super) fn poll_shutdown_progress(&mut self) {
        const MAX_SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

        let Some(progress) = &self.shutdown_progress else {
            return;
        };

        if progress.is_complete() || progress.started_at().elapsed() > MAX_SHUTDOWN_WAIT {
            self.exit_cleanup_complete = true;
            self.release_active_session_lease();
            std::process::exit(0);
        }
    }

    #[profiling::function]
    pub(super) fn render_shutdown_overlay(&self, ctx: &Context) {
        let Some(progress) = &self.shutdown_progress else {
            return;
        };
        let completed = progress.terminals_completed();
        let total = progress.terminal_count();

        egui::CentralPanel::default().show(ctx, |ui| {
            if total > 0 {
                loading_spinner::show_with_detail(
                    ui,
                    egui::Id::new("shutdown_spinner"),
                    "Closing Horizon\u{2026}",
                    &format!("{completed} / {total} terminals shut down"),
                );
            } else {
                loading_spinner::show(ui, egui::Id::new("shutdown_spinner"), Some("Closing Horizon\u{2026}"));
            }
        });
    }

    /// Synchronous fallback for the `on_exit` eframe callback.
    #[profiling::function]
    pub(super) fn run_exit_cleanup(&mut self) {
        if self.exit_cleanup_complete {
            return;
        }

        self.exit_cleanup_complete = true;
        self.auto_save_runtime_state();
        self.board.shutdown_terminal_panels();
        self.git_watchers.clear();
        self.release_active_session_lease();
    }

    #[profiling::function]
    pub(super) fn prepare_frame(&mut self, ctx: &Context) -> bool {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }

        if !self.poll_startup_bootstrap() {
            super::session::render_loading_view(ctx);
            ctx.request_repaint_after(Duration::from_millis(16));
            return false;
        }

        if self.startup_chooser.is_none() && !self.initial_pan_done {
            self.seed_initial_pan(ctx);
        }

        true
    }

    #[profiling::function]
    fn seed_initial_pan(&mut self, ctx: &Context) {
        self.initial_pan_done = true;
        if let Some(workspace_id) = self.leftmost_workspace_id() {
            self.board.focus_workspace(workspace_id);
            if let Some((min, _max)) = self.board.workspace_bounds(workspace_id) {
                let canvas_rect = self.canvas_rect(ctx);
                self.canvas_view.align_canvas_point_to_screen(
                    [canvas_rect.min.x, canvas_rect.min.y],
                    [min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT],
                    [canvas_rect.min.x + 40.0, canvas_rect.center().y],
                );
            }
        }
    }

    #[profiling::function]
    pub(super) fn process_frame_inputs(&mut self, ctx: &Context) -> bool {
        self.handle_fullscreen_toggle(ctx);
        self.handle_shortcuts(ctx);
        self.handle_root_file_drop(ctx);
        let had_terminal_output = self.board.process_output();

        for panel_id in self.board.exited_panels() {
            self.panels_to_close.push(panel_id);
        }

        self.animate_pan(ctx);
        self.maybe_refresh_session_catalog();
        self.poll_remote_hosts_panels();
        self.poll_git_watchers();
        self.poll_config_reload();

        had_terminal_output
    }

    #[profiling::function]
    fn poll_git_watchers(&mut self) {
        // Collect which workspaces need watchers (have GitChanges panels).
        let mut workspaces_needing_watchers: HashMap<WorkspaceId, Option<std::path::PathBuf>> = HashMap::new();
        for panel in &self.board.panels {
            if panel.kind == PanelKind::GitChanges {
                let cwd = panel
                    .launch_cwd
                    .clone()
                    .or_else(|| self.board.workspace(panel.workspace_id).and_then(|ws| ws.cwd.clone()));
                workspaces_needing_watchers.entry(panel.workspace_id).or_insert(cwd);
            }
        }

        // Start watchers for workspaces that need them.
        for (workspace_id, cwd) in &workspaces_needing_watchers {
            if !self.git_watchers.contains_key(workspace_id)
                && let Some(path) = cwd
            {
                tracing::info!(workspace = workspace_id.0, path = %path.display(), "starting git watcher");
                self.git_watchers.insert(*workspace_id, GitWatcher::start(path.clone()));
            }
        }

        // Poll existing watchers and push updates to panels.
        let updates: Vec<(WorkspaceId, std::sync::Arc<horizon_core::GitStatus>)> = self
            .git_watchers
            .iter()
            .filter_map(|(ws_id, watcher)| watcher.try_recv().map(|status| (*ws_id, status)))
            .collect();

        for (workspace_id, status) in updates {
            for panel in &mut self.board.panels {
                if panel.workspace_id == workspace_id
                    && panel.kind == PanelKind::GitChanges
                    && let Some(viewer) = panel.content.git_changes_mut()
                {
                    viewer.update(std::sync::Arc::clone(&status));
                }
            }
        }

        // Remove watchers for workspaces that no longer have GitChanges panels.
        self.git_watchers
            .retain(|ws_id, _| workspaces_needing_watchers.contains_key(ws_id));
    }

    #[profiling::function]
    fn poll_config_reload(&mut self) {
        // Skip while settings editor is open (it manages its own save/reload).
        if self.settings.is_some() {
            return;
        }

        // Check at most every 2 seconds.
        let now = Instant::now();
        if self
            .config_last_check
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(2))
        {
            return;
        }
        self.config_last_check = Some(now);

        let current_mtime = std::fs::metadata(&self.config_path)
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mtime == self.config_last_mtime {
            return;
        }
        self.config_last_mtime = current_mtime;

        if let Ok(config) = Config::load(Some(&self.config_path)) {
            tracing::info!("config file changed, reloading presets");
            self.apply_runtime_config(&config);
            self.board.sync_workspace_metadata(&config);
        }
    }

    #[profiling::function]
    pub(super) fn apply_panel_transitions(&mut self) {
        let panels_to_close = std::mem::take(&mut self.panels_to_close);
        for panel_id in panels_to_close {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_grid_cache.remove(&panel_id);
            self.editor_preview_cache.remove(&panel_id);
            if self.renaming_panel == Some(panel_id) {
                self.clear_panel_rename();
            }
        }
        let panels_to_restart = std::mem::take(&mut self.panels_to_restart);
        for panel_id in panels_to_restart {
            if let Err(error) = self.board.restart_panel(panel_id) {
                tracing::error!(panel_id = panel_id.0, %error, "failed to restart panel");
            } else {
                self.terminal_grid_cache.remove(&panel_id);
                self.editor_preview_cache.remove(&panel_id);
            }
        }
    }

    #[profiling::function]
    pub(super) fn normalize_workspace_state(&mut self, ctx: &Context) {
        let count_before = self.board.workspaces.len();
        self.board.remove_empty_workspaces();
        let count_after = self.board.workspaces.len();
        let detached_before = self.detached_workspaces.len();
        self.detached_workspaces
            .retain(|local_id, _| self.board.workspace_id_by_local_id(local_id).is_some());
        if self.detached_workspaces.len() != detached_before {
            self.mark_runtime_dirty();
        }
        self.pending_detached_window_position_restore
            .retain(|local_id| self.detached_workspaces.contains_key(local_id));
        if self.board.workspaces.is_empty() {
            self.reset_view(ctx);
        } else if count_after < count_before && count_after == 1 {
            let workspace_id = self.board.workspaces[0].id;
            self.board.focus_workspace(workspace_id);
            if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                self.focus_workspace_bounds(ctx, min, max, true);
            }
        }
        if self
            .renaming_workspace
            .is_some_and(|workspace_id| self.board.workspace(workspace_id).is_none())
        {
            self.clear_workspace_rename();
        }
        if self
            .renaming_panel
            .is_some_and(|panel_id| self.board.panel(panel_id).is_none())
        {
            self.clear_panel_rename();
        }
    }

    #[profiling::function]
    pub(super) fn apply_pending_workspace_changes(&mut self) {
        for panel_id in self.workspace_creates.drain(..) {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let workspace_id = self.board.create_workspace(&name);
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
        for (panel_id, workspace_id) in self.workspace_assignments.drain(..) {
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
        self.apply_pending_session_rebinds();
    }

    #[profiling::function]
    pub(super) fn render_active_view(&mut self, ctx: &Context) {
        if self.fullscreen_panel.is_some() {
            self.render_fullscreen_panel(ctx);
            return;
        }

        // Settings side panel renders first so egui reserves the space
        // before the canvas `CentralPanel` claims the remainder.
        if self.settings.is_some() {
            self.render_settings(ctx);
        }

        let workspace_bounds = self.board.workspace_bounds_map();
        self.handle_canvas_pan(ctx);
        self.render_toolbar(ctx);
        self.render_sidebar(ctx);
        self.render_canvas(ctx);
        let overlay_zones = self.overlay_exclusion_zones(ctx);
        self.render_workspace_backgrounds(ctx, &workspace_bounds, &overlay_zones);
        self.handle_canvas_double_click(ctx);
        self.render_panels(ctx);
        self.render_preset_picker(ctx);
        let minimap_height = self.render_minimap(ctx, &workspace_bounds);
        if self.fixed_overlays_visible() && self.template_config.features.attention_feed {
            let feed_result =
                attention_feed::render_attention_feed(ctx, &self.board, minimap_height, &self.template_config.overlays);
            for attention_id in feed_result.dismissed_ids {
                let _ = self.board.dismiss_attention(attention_id);
            }
            if let Some(panel_id) = feed_result.focus_panel {
                self.board.focus(panel_id);
                if let Some(ws_id) = self.board.panel(panel_id).map(|p| p.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(ws_id)
                {
                    self.focus_workspace_bounds(ctx, min, max, true);
                }
            }
        }
        self.render_canvas_hud(ctx);
        self.render_detached_viewports(ctx);
    }

    #[profiling::function]
    pub(super) fn finalize_frame(
        &mut self,
        ctx: &Context,
        had_terminal_output: bool,
        workspace_count_before: usize,
        panel_count_before: usize,
    ) {
        self.render_dir_picker(ctx);
        self.render_command_palette(ctx);
        self.sync_window_config(ctx);
        self.refresh_active_session_lease();

        if self.board.workspaces.len() != workspace_count_before || self.board.panels.len() != panel_count_before {
            self.auto_save_runtime_state();
        }
        self.flush_runtime_if_dirty();

        let has_live_terminals = !self.board.panels.is_empty();
        let animating = self.pan_target.is_some();
        if animating {
            ctx.request_repaint();
        } else if has_live_terminals {
            // Keep streaming terminals responsive, but progressively back off
            // once the board has been quiet for a while to reduce idle CPU.
            let now = Instant::now();
            let poll = if had_terminal_output {
                self.last_terminal_output_at = Some(now);
                Duration::from_millis(16)
            } else {
                let idle_for = self
                    .last_terminal_output_at
                    .map_or(Duration::MAX, |last_output| now.saturating_duration_since(last_output));

                if idle_for < Duration::from_secs(1) {
                    Duration::from_millis(100)
                } else if idle_for < Duration::from_secs(5) {
                    Duration::from_millis(250)
                } else if idle_for < Duration::from_secs(30) {
                    Duration::from_millis(500)
                } else {
                    Duration::from_secs(1)
                }
            };
            ctx.request_repaint_after(poll);
        }
    }
}
