use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Config, GitWatcher, PanelId, PanelKind, WorkspaceId};

use super::super::input;
use crate::theme;

use super::canvas::CanvasGridCache;
use super::{HorizonApp, attention_feed};

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
        let resolved_theme = theme::resolve_theme(self.appearance_theme, ui.system_theme());
        if !self.theme_applied || resolved_theme != self.resolved_theme {
            self.resolved_theme = theme::apply(ui, self.appearance_theme);
            self.theme_applied = true;
            self.panel_render_caches.terminal_grid_cache.clear();
            self.canvas_grid_cache = CanvasGridCache::default();
            self.panel_render_caches.editor_preview_cache.clear();
            self.panel_render_caches.browser_ui_state.clear();
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
        self.sync_panel_focus_from_pointer_press(ctx);
        // Speech runs before the fullscreen handler so that Escape cancels an
        // active recording instead of also exiting panel fullscreen.
        self.handle_speech_input(ctx);
        self.handle_fullscreen_toggle(ctx);
        self.handle_shortcuts(ctx);
        self.handle_root_file_drop(ctx);
        let had_panel_output = self.drain_panel_output();

        self.animate_pan(ctx);
        self.poll_primary_selection_paste();
        self.maybe_refresh_session_catalog();
        self.poll_remote_hosts_refresh();
        self.poll_ssh_upload_flow();
        self.poll_git_watchers();
        self.poll_config_reload();
        self.poll_update_check();
        self.maybe_start_update_check();

        had_panel_output
    }

    /// Drain terminal and browser events, promoting persistence-relevant
    /// changes into the app's runtime dirty state.
    pub(super) fn drain_panel_output(&mut self) -> bool {
        let panel_output = self.board.process_output();
        if panel_output.cwd_changed || panel_output.persisted_state_changed {
            self.mark_runtime_dirty();
        }
        panel_output.activity.terminal || panel_output.activity.browser
    }

    fn poll_primary_selection_paste(&mut self) {
        while let Some(paste) = self.primary_selection.try_recv_paste() {
            let Some(panel) = self.board.panel_mut(paste.panel_id) else {
                continue;
            };
            let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
                continue;
            };
            let bytes = input::paste_bytes(&paste.text, mode, true);
            panel.write_input(&bytes);
        }
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

    pub(super) fn queue_panel_restart(&mut self, panel_id: PanelId) {
        if self.cancel_speech_target(panel_id) {
            tracing::info!(panel_id = panel_id.0, "active dictation cancelled before panel restart");
        }
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
        for panel_id in self.workspace_creates.drain(..) {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let workspace_id = self.board.create_workspace(&name);
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
        for (panel_id, workspace_id) in self.workspace_assignments.drain(..) {
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
    }

    #[profiling::function]
    pub(super) fn render_active_view(&mut self, ui: &mut egui::Ui, root_interaction_suppressed: bool) {
        if self.fullscreen_panel.is_some() {
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

        let workspace_bounds = self.board.workspace_bounds_map();
        if !root_interaction_suppressed {
            self.handle_canvas_pan(ui);
        }
        self.render_toolbar(ui);
        self.render_sidebar(ui);
        self.render_canvas(ui);
        let overlay_zones = self.overlay_exclusion_zones(ui);
        self.render_workspace_backgrounds(ui, &workspace_bounds, &overlay_zones);
        self.render_empty_state_card(ui);
        self.handle_canvas_double_click(ui);
        self.render_panels(ui);
        self.render_file_drop_highlight(ui);
        self.render_preset_picker(ui);
        let minimap_height = self.render_minimap(ui, &workspace_bounds);
        if self.fixed_overlays_visible() && self.template_config.features.attention_feed {
            let feed_result =
                attention_feed::render_attention_feed(ui, &self.board, minimap_height, &self.template_config.overlays);
            for attention_id in feed_result.dismissed_ids {
                let _ = self.board.dismiss_attention(attention_id);
            }
            if let Some(panel_id) = feed_result.focus_panel {
                self.reveal_panel_visible(ui, panel_id);
            }
        }
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
        self.cancel_unattended_recording();
        self.render_dir_picker(ctx);
        self.render_command_palette(ctx);
        self.render_remote_hosts_overlay(ctx);
        self.render_session_manager(ctx);
        self.render_ssh_upload_flow(ctx);
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

        let has_live_panels = !self.board.panels.is_empty();
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
    use crate::test_egui::DiscardTextures;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[cfg(feature = "speech")]
    use std::time::Duration;

    use egui::Context;
    #[cfg(feature = "speech")]
    use horizon_core::PanelId;

    #[cfg(feature = "speech")]
    use crate::app::HeldSpeechBinding;
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

    #[cfg(feature = "speech")]
    #[test]
    fn empty_board_keeps_polling_preloads_and_publishes_success_in_the_drain_frame() {
        let ctx = Context::default();
        let (_temp, mut app) = test_app();
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        app.theme_applied = true;
        // Viewport settling has its own wall-clock tests. Keep this test scoped
        // to the speech preload deadline instead of coupling it to that timer.
        app.root_viewport_stabilizer = None;
        assert!(app.board.panels.is_empty());

        let mut frame = eframe::Frame::_new_kittest();
        // Warm the context once; a context's first pass requests an immediate
        // repaint for its own initialization, which masks later deadlines.
        // Honor immediate follow-up passes like an integration backend until
        // the app exposes its next delayed deadline.
        let mut repaint_delay = Duration::ZERO;
        for _ in 0..32 {
            let output = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    eframe::App::ui(&mut app, ui, &mut frame);
                })
                .discard_textures();
            repaint_delay = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .expect("root viewport output")
                .repaint_delay;
            if !repaint_delay.is_zero() {
                break;
            }
        }

        assert!(
            app.speech
                .as_ref()
                .is_some_and(super::super::speech::SpeechSystem::has_pending_preloads)
        );
        assert!(!repaint_delay.is_zero());
        assert!(repaint_delay <= crate::app::speech::SPEECH_POLL_INTERVAL);

        channels.complete_preload("Metal");
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                eframe::App::ui(&mut app, ui, &mut frame);
            })
            .discard_textures();

        assert!(
            !app.speech
                .as_ref()
                .is_some_and(super::super::speech::SpeechSystem::has_pending_preloads)
        );
        assert_eq!(
            ctx.data(|data| data.get_temp::<String>(egui::Id::new("speech_active_backend"))),
            Some("Metal".to_string())
        );
    }

    #[cfg(feature = "speech")]
    #[test]
    fn startup_chooser_drains_and_renders_preload_failure() {
        use horizon_core::{StartupChooser, StartupPromptReason};

        let ctx = Context::default();
        let (_temp, mut app) = test_app();
        app.startup_chooser = Some(crate::app::StartupChooserState::new(StartupChooser {
            reason: StartupPromptReason::LiveConflict,
            config_path: "/tmp/horizon-config.yaml".to_string(),
            sessions: Vec::new(),
        }));
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        channels.fail_preload("model not found");

        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                eframe::App::ui(&mut app, ui, &mut frame);
            })
            .discard_textures();

        assert!(app.startup_chooser.is_some());
        assert!(app.speech_notice.as_ref().is_some_and(|notice| {
            notice.error && notice.message.contains("profile 1") && notice.message.contains("model not found")
        }));
        assert!(
            !app.speech
                .as_ref()
                .is_some_and(super::super::speech::SpeechSystem::has_pending_preloads)
        );
    }

    #[cfg(feature = "speech")]
    #[test]
    fn startup_loading_view_drains_and_renders_preload_failure() {
        let ctx = Context::default();
        let (_temp, mut app) = test_app();
        let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
        app.startup_receiver = Some(bootstrap_rx);
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        channels.fail_preload("invalid model");

        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                eframe::App::ui(&mut app, ui, &mut frame);
            })
            .discard_textures();

        assert!(app.startup_receiver.is_some());
        assert!(app.speech_notice.as_ref().is_some_and(|notice| {
            notice.error && notice.message.contains("profile 1") && notice.message.contains("invalid model")
        }));
        assert!(
            !app.speech
                .as_ref()
                .is_some_and(super::super::speech::SpeechSystem::has_pending_preloads)
        );
        drop(bootstrap_tx);
    }

    #[test]
    fn long_speech_notice_stays_within_a_narrow_viewport() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(880.0, 632.0));
        let ctx = Context::default();
        let (_temp, mut app) = test_app();
        app.show_speech_notice(
            "Speech input error: preloading the `Default` speech model failed: \
             failed to load a model from a deliberately long temporary path because the file was not found",
            true,
        );
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..egui::RawInput::default()
                },
                |ui| app.render_speech_notice(ui),
            )
            .discard_textures();
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..egui::RawInput::default()
                },
                |ui| app.render_speech_notice(ui),
            )
            .discard_textures();
        let bounds = output
            .shapes
            .iter()
            .map(|shape| shape.shape.visual_bounding_rect())
            .filter(|bounds| bounds.is_finite() && bounds.is_positive())
            .reduce(egui::Rect::union)
            .expect("speech notice should paint");

        assert!(screen.expand(1.0).contains_rect(bounds), "notice bounds {bounds:?}");
    }

    #[cfg(feature = "speech")]
    #[test]
    fn panel_restart_cancels_only_targeted_speech_and_retains_held_binding() {
        let (_temp, mut app) = test_app();
        let target = PanelId(7);
        let other = PanelId(8);
        let chord = horizon_core::ShortcutBinding::parse("F1").expect("valid test shortcut");
        let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
        speech.start(crate::app::speech::SpeechSink::Panel(target), 0);
        assert!(channels.capture_start_requested());
        app.speech = Some(speech);
        app.speech_engaged_profile = Some(0);
        app.speech_held_bindings.push(HeldSpeechBinding::new(chord));

        app.queue_panel_restart(other);

        assert_eq!(
            app.speech
                .as_ref()
                .and_then(super::super::speech::SpeechSystem::active_target),
            Some(target)
        );
        assert_eq!(app.speech_engaged_profile, Some(0));

        app.queue_panel_restart(target);
        app.queue_panel_restart(target);

        assert!(
            app.speech
                .as_ref()
                .and_then(super::super::speech::SpeechSystem::active_target)
                .is_none()
        );
        assert!(app.speech_engaged_profile.is_none());
        assert_eq!(app.panels_to_restart, vec![other, target]);
        assert_eq!(app.speech_held_bindings.len(), 1);
        assert_eq!(app.speech_held_bindings[0].binding, chord);
        assert!(app.speech_held_bindings[0].release_deadline.is_none());
    }
}
