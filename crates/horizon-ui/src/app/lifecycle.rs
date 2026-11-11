use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{ClipboardTarget, GitWatcher, PanelId, PanelKind, WorkspaceId};

use super::super::input;
use crate::primary_selection::PrimarySelectionPaste;
use crate::theme;

use super::HorizonApp;
use super::canvas::CanvasGridCache;
use super::canvas_scroll::reset_canvas_scroll;

mod shutdown;
mod startup_workspace;

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

    #[profiling::function]
    pub(super) fn prepare_frame(&mut self, ui: &mut egui::Ui) -> bool {
        super::bootstrap::pin_chrome_to_native_display_scale(ui.ctx());
        let resolved_theme = theme::resolve_theme(self.appearance_theme, ui.system_theme());
        if !self.theme_applied || resolved_theme != self.resolved_theme {
            self.resolved_theme = theme::apply(ui, self.appearance_theme);
            self.theme_applied = true;
            self.panel_render_caches.terminal_grid_cache.clear();
            self.canvas_grid_cache = CanvasGridCache::default();
            self.panel_render_caches.editor_preview_cache.clear();
            for state in self.panel_render_caches.browser_ui_state.values_mut() {
                state.reset_rendering();
            }
        }

        if !self.prepare_startup_bootstrap(ui) {
            return false;
        }

        if self.startup_chooser.is_none() && !self.initial_pan_done && !self.startup_workspace_organization_pending {
            self.seed_initial_pan(ui, None, false);
        }

        true
    }

    #[profiling::function]
    pub(super) fn process_frame_inputs(&mut self, ctx: &Context) -> bool {
        self.poll_saved_session_deletion(ctx);
        self.filter_held_navigation_keys(ctx);
        let input_blocked = self.host_dialog_open()
            || super::panels::session_picker_panel(ctx).is_some_and(|panel| self.board.panel(panel).is_some());
        if input_blocked {
            self.clear_file_drop_state(ctx);
        } else {
            self.sync_panel_focus_from_pointer_press(ctx);
        }
        if !input_blocked {
            self.handle_fullscreen_toggle(ctx);
            self.handle_shortcuts(ctx);
            self.handle_root_file_drop(ctx);
            self.poll_primary_selection_paste();
        }
        let had_panel_output = self.drain_panel_output();
        self.poll_reconnects(ctx, Instant::now());
        self.forward_terminal_clipboard_writes(ctx);
        let browser_create_activity = self.poll_browser_create_requests(ctx);
        let device_activity = self.poll_device_panel_requests(ctx);
        let block_activity = self.poll_block_panel_requests(ctx);

        self.animate_pan(ctx);
        self.maybe_refresh_session_catalog(ctx);
        self.poll_remote_hosts_refresh();
        self.poll_ssh_upload_flow();
        self.poll_git_watchers();
        self.poll_config_reload();

        had_panel_output || browser_create_activity || device_activity || block_activity
    }

    /// Drain terminal and browser events, promoting persistence-relevant
    /// changes into the app's runtime dirty state.
    ///
    /// Returns whether the output needs a fast repaint. Terminal output counts
    /// only for panels drawn in the previous frame, so a busy agent in a culled
    /// or hidden panel does not hold the window at the output frame rate. An
    /// answered terminal query always counts, because the program can wait
    /// for the answer before it sends its next query.
    pub(super) fn drain_panel_output(&mut self) -> bool {
        let panel_output = self.board.process_output();
        let mut on_screen_terminal_output = false;
        if panel_output.activity.terminal {
            // Output can arrive while a panel is culled. Invalidate now, before
            // the next quiet poll clears its one-frame activity flag.
            for panel in &self.board.panels {
                if panel.had_recent_output() {
                    self.panel_render_caches.terminal_grid_cache.remove(&panel.id);
                    on_screen_terminal_output |= self.panel_drawn_last_frame(panel.id);
                }
            }
        }
        if panel_output.cwd_changed || panel_output.persisted_state_changed {
            self.mark_runtime_dirty();
        }
        on_screen_terminal_output || panel_output.activity.answered_query || panel_output.activity.browser
    }

    /// Whether the previous frame drew this panel in the root window, as the
    /// fullscreen panel, or in a detached workspace window.
    pub(in crate::app) fn panel_drawn_last_frame(&self, panel_id: PanelId) -> bool {
        let drawn_in_root = match self.fullscreen_panel {
            Some(fullscreen_panel) => fullscreen_panel == panel_id,
            None => self.panel_screen_rects.contains_key(&panel_id),
        };
        drawn_in_root
            || self
                .detached_workspaces
                .values()
                .any(|detached| detached.panel_screen_rects.contains_key(&panel_id))
    }

    /// Hand OSC 52 copy requests from terminal panels to the system, so TUIs
    /// that have no `wl-copy`/`xclip`/`pbcopy` to call still reach it.
    fn forward_terminal_clipboard_writes(&mut self, ctx: &Context) {
        for write in self.board.take_terminal_clipboard_writes() {
            match write.target {
                ClipboardTarget::Clipboard => ctx.copy_text(write.text),
                ClipboardTarget::Selection => self.primary_selection.copy(&write.text),
            }
        }
    }

    fn poll_primary_selection_paste(&mut self) {
        while let Some(paste) = self.primary_selection.try_recv_paste() {
            self.deliver_primary_paste(&paste);
        }
    }

    /// Writes a middle-click paste to its terminal.
    pub(super) fn deliver_primary_paste(&mut self, paste: &PrimarySelectionPaste) {
        let Some(panel) = self.board.panel_mut(paste.panel_id) else {
            return;
        };
        let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
            return;
        };
        let bytes = input::paste_bytes(&paste.text, mode, true);
        panel.write_input(&bytes);
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

        let current_mtime = super::util::settings_mtime(&self.config_path);

        if current_mtime == self.config_last_mtime {
            return;
        }
        if self.settings_reload_pending {
            // One read at a time; the reply applies the settings.
            return;
        }
        self.config_last_mtime = current_mtime;
        self.settings_reload_pending = true;
        // The read waits on the settings thread, and its reply applies the
        // settings in a later frame.
        self.settings_io.reload(self.config_path.clone());
    }

    /// Restarts a panel now, because the person picked its "Reconnect" action
    /// or pressed a key in a terminal that waits for its next attempt.
    pub(super) fn reconnect_panel_now(&mut self, panel_id: PanelId) {
        self.reconnects.cancel(panel_id);
        self.queue_panel_restart(panel_id);
    }

    pub(super) fn queue_panel_restart(&mut self, panel_id: PanelId) {
        if !self.panels_to_restart.contains(&panel_id) {
            self.panels_to_restart.push(panel_id);
        }
    }

    #[profiling::function]
    pub(super) fn apply_panel_transitions(&mut self) {
        let panels_to_close = std::mem::take(&mut self.panels_to_close);
        for panel_id in panels_to_close {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_body_screen_rects.remove(&panel_id);
            self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
            self.panel_render_caches.editor_preview_cache.remove(&panel_id);
            self.panel_render_caches.browser_ui_state.remove(&panel_id);
            self.panel_render_caches.device_ui_state.remove(&panel_id);
            if self.renaming_panel == Some(panel_id) {
                self.clear_panel_rename();
            }
        }
        let panels_to_restart = std::mem::take(&mut self.panels_to_restart);
        for panel_id in panels_to_restart {
            if let Err(error) = self.board.restart_panel(panel_id) {
                tracing::error!(panel_id = panel_id.0, %error, "failed to restart panel");
            } else {
                self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
                self.panel_render_caches.editor_preview_cache.remove(&panel_id);
                self.panel_render_caches.browser_ui_state.remove(&panel_id);
                self.panel_render_caches.device_ui_state.remove(&panel_id);
            }
        }
    }

    #[profiling::function]
    pub(super) fn normalize_workspace_state(&mut self, ctx: &Context) {
        // A card that a saved record brings back holds its empty workspace, so it comes before
        // the cleanup that would take the workspace away. A worker thread reads the records, so
        // the cleanup waits the frames of that read instead of the frame waiting for the file.
        let records_wait = false;
        let count_before = self.board.workspaces.len();
        if !records_wait {
            self.board.remove_empty_workspaces();
        }
        let count_after = self.board.workspaces.len();
        let detached_before = self.detached_workspaces.len();
        self.detached_workspaces
            .retain(|local_id, _state| self.board.workspace_id_by_local_id(local_id).is_some());
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
        for panel_id in std::mem::take(&mut self.workspace_creates) {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let workspace_id = self.board.create_workspace(&name);
            self.assign_panel_workspace_and_mark_dirty(panel_id, workspace_id);
        }
        for (panel_id, workspace_id) in std::mem::take(&mut self.workspace_assignments) {
            self.assign_panel_workspace_and_mark_dirty(panel_id, workspace_id);
        }
    }

    fn assign_panel_workspace_and_mark_dirty(&mut self, panel_id: PanelId, workspace_id: WorkspaceId) {
        let previous_workspace = self.board.panel_workspace_id(panel_id);
        self.board.assign_panel_to_workspace(panel_id, workspace_id);
        if self.board.panel_workspace_id(panel_id) != previous_workspace {
            self.mark_runtime_dirty();
        }
    }

    #[profiling::function]
    pub(super) fn render_active_view(&mut self, ui: &mut egui::Ui, root_interaction_suppressed: bool) {
        self.process_pending_detached_reattach(ui.ctx());
        self.apply_pending_root_device_reveal(ui.ctx());

        if self.fullscreen_panel.is_some() {
            reset_canvas_scroll(ui.ctx());
            self.render_fullscreen_panel(ui);
            // Detached windows are immediate viewports: egui closes any child
            // viewport that is not shown during a pass, so they must keep
            // rendering while a panel is fullscreen in the root window.
            self.render_detached_viewports(ui);
            return;
        }

        // Settings side panel renders first so egui reserves the space
        // before the canvas `CentralPanel` claims the remainder.
        if self.settings.is_some() {
            self.render_settings(ui);
        }

        let mut workspace_bounds = self.board.workspace_bounds_map();
        let workspace_bounds = &mut workspace_bounds;
        if !root_interaction_suppressed && !self.host_dialog_open() {
            self.handle_canvas_pan(ui);
        } else {
            reset_canvas_scroll(ui.ctx());
        }
        self.render_toolbar(ui);
        self.render_work_resume_banner(ui);
        self.render_sidebar(ui);
        self.render_canvas(ui);
        let overlay_zones = self.overlay_exclusion_zones(ui);
        self.render_workspace_backgrounds(ui, workspace_bounds, &overlay_zones);
        self.render_browser_connector_lines(ui.ctx(), self.canvas_rect(ui.ctx()), None);
        self.render_empty_state_card(ui);
        if !self.host_dialog_open() {
            self.handle_canvas_double_click(ui);
        }
        self.render_panels(ui);
        self.render_file_drop_highlight(ui);
        self.render_preset_picker(ui);
        self.render_minimap(ui, workspace_bounds);
        self.render_canvas_hud(ui);
        self.render_detached_viewports(ui);
    }

    #[profiling::function]
    pub(super) fn finalize_frame(
        &mut self,
        ctx: &Context,
        had_panel_output: bool,
        workspace_count_before: usize,
        panel_count_before: usize,
    ) {
        // The pass that drew a card measured its content; the height that fits
        // it follows before the board is saved.
        self.fit_block_cards(ctx);
        self.render_dir_picker(ctx);
        self.render_command_palette(ctx);
        self.render_remote_hosts_overlay(ctx);
        self.render_session_manager(ctx);
        self.render_ssh_upload_flow(ctx);
        self.render_browser_file_chooser(ctx, None);
        self.sync_window_config(ctx);
        self.refresh_active_session_lease();

        if (self.board.workspaces.len() != workspace_count_before || self.board.panels.len() != panel_count_before)
            && !self.auto_save_runtime_state()
        {
            self.mark_runtime_dirty();
        }
        self.flush_runtime_if_dirty();

        if !self.theme_applied {
            // Deferred theme swaps are applied in prepare_frame, so guarantee
            // one follow-up frame even when the UI is otherwise idle.
            ctx.request_repaint();
        }

        let has_live_panels = !self.board.panels.is_empty() || self.board.has_pending_browser_cleanup();
        let animating = self.pan_target.is_some();
        if animating {
            ctx.request_repaint();
        } else if has_live_panels {
            // Keep live panels responsive, but progressively back off
            // once the board has been quiet for a while to reduce idle CPU.
            let now = Instant::now();
            let poll = if had_panel_output {
                self.last_panel_output_at = Some(now);
                Duration::from_millis(16)
            } else {
                let idle_for = self
                    .last_panel_output_at
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

impl Drop for HorizonApp {
    fn drop(&mut self) {
        self.run_exit_cleanup();
    }
}

#[cfg(test)]
#[path = "lifecycle/startup_organization_tests.rs"]
mod startup_organization_tests;

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use egui::Context;

    use crate::app::test_support::test_app;

    #[test]
    fn finalize_frame_requests_repaint_when_theme_application_is_deferred() {
        let ctx = Context::default();
        let (_temp, mut app) = test_app();
        app.theme_applied = false;
        let repaint_requests = Arc::new(AtomicUsize::new(0));
        let repaint_requests_for_callback = Arc::clone(&repaint_requests);
        ctx.set_request_repaint_callback(move |_| {
            repaint_requests_for_callback.fetch_add(1, Ordering::Relaxed);
        });

        app.finalize_frame(&ctx, false, 0, 0);

        assert!(repaint_requests.load(Ordering::Relaxed) > 0);
    }
}

// The fixture uses a Unix shell and PTY.
#[cfg(all(test, unix))]
#[path = "lifecycle/terminal_cache_tests.rs"]
mod terminal_cache_tests;

#[cfg(test)]
#[path = "lifecycle/workspace_persistence_tests.rs"]
mod workspace_persistence_tests;
