use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Config, GitWatcher, PanelKind, WorkspaceId};

use super::super::input;
use crate::{loading_spinner, theme};

use super::canvas::CanvasGridCache;
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

        self.shutdown_speech_runtime();
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
        self.shutdown_speech_runtime();
        self.auto_save_runtime_state();
        self.board.shutdown_terminal_panels();
        self.git_watchers.clear();
        self.release_active_session_lease();
    }

    #[profiling::function]
    pub(super) fn prepare_frame(&mut self, ctx: &Context) -> bool {
        let resolved_theme = theme::resolve_theme(self.appearance_theme, ctx.system_theme());
        if !self.theme_applied || resolved_theme != self.resolved_theme {
            self.resolved_theme = theme::apply(ctx, self.appearance_theme);
            self.theme_applied = true;
            self.terminal_grid_cache.clear();
            self.canvas_grid_cache = CanvasGridCache::default();
            self.editor_preview_cache.clear();
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
        self.sync_panel_focus_from_pointer_press(ctx);
        // Speech runs before the fullscreen handler so that Escape cancels an
        // active recording instead of also exiting panel fullscreen.
        self.handle_speech_input(ctx);
        self.handle_fullscreen_toggle(ctx);
        self.handle_shortcuts(ctx);
        self.handle_root_file_drop(ctx);
        let panel_output = self.board.process_output();
        if panel_output.cwd_changed {
            self.mark_runtime_dirty();
        }
        let had_terminal_output = panel_output.had_terminal_output;

        self.animate_pan(ctx);
        self.poll_primary_selection_paste();
        self.maybe_refresh_session_catalog();
        self.poll_remote_hosts_refresh();
        self.poll_ssh_upload_flow();
        self.poll_git_watchers();
        self.poll_config_reload();
        self.poll_update_check();
        self.maybe_start_update_check();

        had_terminal_output
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

    #[profiling::function]
    pub(super) fn apply_panel_transitions(&mut self) {
        let panels_to_close = std::mem::take(&mut self.panels_to_close);
        for panel_id in panels_to_close {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_body_screen_rects.remove(&panel_id);
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
    pub(super) fn render_active_view(&mut self, ctx: &Context) {
        if self.fullscreen_panel.is_some() {
            self.render_fullscreen_panel(ctx);
            // Detached windows are immediate viewports: egui closes any child
            // viewport that is not shown during a pass, so they must keep
            // rendering while a panel is fullscreen in the root window.
            self.render_detached_viewports(ctx);
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
        self.render_empty_state_card(ctx);
        self.handle_canvas_double_click(ctx);
        self.render_panels(ctx);
        self.render_file_drop_highlight(ctx);
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
        self.finalize_pending_terminal_speech();
        self.cancel_unattended_recording();
        self.render_dir_picker(ctx);
        self.render_command_palette(ctx);
        self.render_remote_hosts_overlay(ctx);
        self.render_session_manager(ctx);
        self.render_ssh_upload_flow(ctx);
        self.sync_window_config(ctx);
        self.refresh_active_session_lease();

        if self.board.workspaces.len() != workspace_count_before || self.board.panels.len() != panel_count_before {
            self.auto_save_runtime_state();
        }
        self.flush_runtime_if_dirty();

        if !self.theme_applied {
            // Deferred theme swaps are applied in prepare_frame, so guarantee
            // one follow-up frame even when the UI is otherwise idle.
            ctx.request_repaint();
        }

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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    use eframe::CreationContext;
    use egui::Context;
    use horizon_core::{Config, HorizonHome, PanelId, RuntimeState, SessionStore, StartupDecision};

    use super::HorizonApp;
    use crate::app::HeldSpeechBinding;
    use crate::app::speech::SpeechTarget;
    #[cfg(feature = "speech")]
    use crate::app::speech::external_text::FocusedTarget;
    #[cfg(feature = "speech")]
    use crate::app::speech::global_hotkeys::GlobalHotkeyEvent;
    use crate::app::speech::lifecycle::{
        HoldHotkeyTransition, SPEECH_RELEASE_OWNERSHIP_TIMEOUT, SpeechActivity, hold_hotkey_transition,
    };
    #[cfg(feature = "speech")]
    use crate::app::speech::lifecycle::{SPEECH_POLL_INTERVAL, handle_profile_hotkeys};
    use crate::input;

    fn test_app() -> HorizonApp {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.yaml");
        let home = HorizonHome::from_root(temp.path().join(".horizon"));
        let session_store = SessionStore::new(home, config_path.clone());
        let config = Config::default();
        let ctx = Context::default();
        let cc = CreationContext::_new_kittest(ctx);

        HorizonApp::new(
            &cc,
            &config,
            config_path,
            session_store,
            StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
            input::ObservedKeyboardInputs::default(),
        )
    }

    #[test]
    fn finalize_frame_requests_repaint_when_theme_application_is_deferred() {
        let ctx = Context::default();
        let mut app = test_app();
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
        let mut app = test_app();
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        app.theme_applied = true;
        assert!(app.board.panels.is_empty());

        let mut frame = eframe::Frame::_new_kittest();
        // Warm the context once; a context's first pass requests an immediate
        // repaint for its own initialization, which masks later deadlines.
        // Honor immediate follow-up passes like an integration backend until
        // the app exposes its next delayed deadline.
        let mut repaint_delay = Duration::ZERO;
        for _ in 0..8 {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                eframe::App::update(&mut app, ctx, &mut frame);
            });
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
        assert!(repaint_delay <= SPEECH_POLL_INTERVAL);

        channels.complete_preload("Metal");
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame);
        });

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
        let mut app = test_app();
        app.startup_chooser = Some(crate::app::StartupChooserState::new(StartupChooser {
            reason: StartupPromptReason::LiveConflict,
            config_path: "/tmp/horizon-config.yaml".to_string(),
            sessions: Vec::new(),
        }));
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        channels.fail_preload("model not found");

        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame);
        });

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
        let mut app = test_app();
        let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
        app.startup_receiver = Some(bootstrap_rx);
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_preload();
        app.speech = Some(speech);
        channels.fail_preload("invalid model");

        let mut frame = eframe::Frame::_new_kittest();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            eframe::App::update(&mut app, ctx, &mut frame);
        });

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
        let mut app = test_app();
        app.show_speech_notice(
            "Speech input error: preloading the `Default` speech model failed: \
             failed to load a model from a deliberately long temporary path because the file was not found",
            true,
        );
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..egui::RawInput::default()
            },
            |ctx| app.render_speech_notice(ctx),
        );
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..egui::RawInput::default()
            },
            |ctx| app.render_speech_notice(ctx),
        );
        let bounds = output
            .shapes
            .iter()
            .map(|shape| shape.shape.visual_bounding_rect())
            .filter(|bounds| bounds.is_finite() && bounds.is_positive())
            .reduce(egui::Rect::union)
            .expect("speech notice should paint");

        assert!(screen.expand(1.0).contains_rect(bounds), "notice bounds {bounds:?}");
    }

    /// Root focus may move directly into a detached Horizon viewport. Keep
    /// ownership across a transient all-unfocused pass, but bound it in case
    /// the destination viewport never receives the key-up.
    #[test]
    fn focus_loss_ownership_survives_handoff_and_expires_without_key_up() {
        let ctx = Context::default();
        let now = Instant::now();
        let mut app = test_app();
        let chord = horizon_core::ShortcutBinding::new(
            horizon_core::ShortcutModifiers::CTRL,
            horizon_core::ShortcutKey::Letter('K'),
        );
        // Pressed with no terminal focused: the filter holds the chord, but
        // the engine never engaged a profile.
        app.speech_held_bindings.push(HeldSpeechBinding::new(chord));
        app.speech_engaged_profile = None;
        app.speech_escape_release_pending = true;

        app.stop_hold_on_focus_loss(&ctx, now);

        assert_eq!(app.speech_held_bindings.len(), 1);
        assert_eq!(app.speech_held_bindings[0].binding, chord);
        assert!(app.speech_escape_release_pending);
        assert_eq!(
            app.speech_held_bindings[0].release_deadline,
            Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
        );
        assert_eq!(
            app.speech_escape_release_deadline,
            Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
        );

        app.any_viewport_focused = false;
        app.cancel_unattended_recording();

        assert_eq!(app.speech_held_bindings.len(), 1);
        assert!(app.speech_escape_release_pending);

        let later = now + Duration::from_secs(1);
        let newer_chord = horizon_core::ShortcutBinding::new(
            horizon_core::ShortcutModifiers::ALT,
            horizon_core::ShortcutKey::Letter('L'),
        );
        app.speech_held_bindings.push(HeldSpeechBinding::new(newer_chord));
        // A newly consumed Escape is a separate ownership generation.
        app.speech_escape_release_deadline = None;
        app.arm_speech_release_ownership(&ctx, later);

        assert_eq!(
            app.speech_held_bindings[0].release_deadline,
            Some(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
        );
        assert_eq!(
            app.speech_held_bindings[1].release_deadline,
            Some(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
        );
        assert_eq!(
            app.speech_escape_release_deadline,
            Some(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT)
        );

        app.expire_speech_release_ownership(now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT);

        assert_eq!(app.speech_held_bindings.len(), 1);
        assert_eq!(app.speech_held_bindings[0].binding, newer_chord);
        assert!(app.speech_escape_release_pending);

        app.expire_speech_release_ownership(later + SPEECH_RELEASE_OWNERSHIP_TIMEOUT);

        assert!(app.speech_held_bindings.is_empty());
        assert!(!app.speech_escape_release_pending);
        assert!(app.speech_escape_release_deadline.is_none());
    }

    #[test]
    fn terminal_result_waiting_for_viewport_focus_is_discarded_when_all_windows_are_unfocused() {
        let mut app = test_app();
        app.any_viewport_focused = false;
        app.pending_terminal_speech
            .push((PanelId(7), "private late transcript".to_string()));

        app.finalize_pending_terminal_speech();

        assert!(app.pending_terminal_speech.is_empty());
    }

    #[cfg(feature = "speech")]
    #[test]
    fn horizon_focus_loss_preserves_external_hold_release_ownership() {
        let mut app = test_app();
        let (mut speech, _channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
        let target = SpeechTarget::External(crate::app::speech::ExternalTargetId::from_raw(9));
        assert!(speech.start(target, 0));
        app.speech = Some(speech);
        app.speech_engaged_profile = Some(0);
        app.any_viewport_focused = false;

        app.cancel_unattended_recording();

        assert_eq!(app.speech_engaged_profile, Some(0));
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            Some(target)
        );
    }

    #[cfg(feature = "speech")]
    #[test]
    fn global_external_hold_dispatch_owns_only_its_profile_until_matching_release() {
        use egui::{Event, Key, Modifiers, RawInput};

        let mut app = test_app();
        let (speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1", "F2"]);
        app.speech = Some(speech);
        let external_id = crate::app::speech::ExternalTargetId::from_raw(41);
        let target = SpeechTarget::External(external_id);
        app.speech_external_targets
            .inject_capture(Ok(FocusedTarget::External(external_id)));
        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 7,
            profile: 0,
            pressed: true,
        });
        let mut events = Vec::new();

        app.handle_injected_global_speech_events(&mut events);

        assert_eq!(app.speech_engaged_profile, Some(0));
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            Some(target)
        );
        assert!(channels.capture_start_requested());

        // A mirrored egui press is gated while Carbon owns the registered
        // binding, so the same physical F1 press cannot start capture twice.
        let ctx = Context::default();
        let mirrored_press = RawInput {
            events: vec![Event::Key {
                key: Key::F1,
                physical_key: Some(Key::F1),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        };
        let mut mirrored_events = Vec::new();
        let mut engaged = app.speech_engaged_profile;
        let _ = ctx.run(mirrored_press, |ctx| {
            engaged = handle_profile_hotkeys(
                ctx,
                app.speech.as_mut().expect("speech runtime"),
                None,
                false,
                engaged,
                &mut mirrored_events,
            );
        });
        app.speech_engaged_profile = engaged;
        assert_eq!(app.speech_engaged_profile, Some(0));
        assert!(!channels.capture_start_requested());
        assert!(mirrored_events.is_empty());

        // A second profile may press and release while F1 owns recording,
        // but neither event may steal ownership or stop the first profile.
        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 7,
            profile: 1,
            pressed: true,
        });
        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 7,
            profile: 1,
            pressed: false,
        });
        app.handle_injected_global_speech_events(&mut events);
        assert_eq!(app.speech_engaged_profile, Some(0));
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            Some(target)
        );
        assert_eq!(events.len(), 1);

        // Horizon being backgrounded is expected for an external target;
        // the matching global release remains responsible for ending hold.
        app.any_viewport_focused = false;
        app.cancel_unattended_recording();
        assert_eq!(app.speech_engaged_profile, Some(0));
        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 7,
            profile: 0,
            pressed: false,
        });
        app.handle_injected_global_speech_events(&mut events);
        assert_eq!(app.speech_engaged_profile, None);
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            None
        );
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::active_target),
            Some(target)
        );
    }

    #[cfg(feature = "speech")]
    #[test]
    fn global_external_toggle_ignores_release_and_any_press_stops_recording() {
        let mut app = test_app();
        let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1", "F2"]);
        speech.set_test_hotkey_mode(horizon_core::SpeechHotkeyMode::Toggle);
        app.speech = Some(speech);
        let external_id = crate::app::speech::ExternalTargetId::from_raw(42);
        let target = SpeechTarget::External(external_id);
        app.speech_external_targets
            .inject_capture(Ok(FocusedTarget::External(external_id)));
        let mut events = Vec::new();

        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 9,
            profile: 1,
            pressed: true,
        });
        app.handle_injected_global_speech_events(&mut events);
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            Some(target)
        );
        assert!(channels.capture_start_requested());

        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 9,
            profile: 1,
            pressed: false,
        });
        app.handle_injected_global_speech_events(&mut events);
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            Some(target)
        );

        app.speech_global_hotkeys.inject_event(GlobalHotkeyEvent {
            generation: 9,
            profile: 0,
            pressed: true,
        });
        app.handle_injected_global_speech_events(&mut events);
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::recording_target),
            None
        );
        assert_eq!(
            app.speech
                .as_ref()
                .and_then(crate::app::speech::SpeechSystem::active_target),
            Some(target)
        );
        assert_eq!(app.speech_engaged_profile, None);
        assert!(events.is_empty());
        assert!(!channels.capture_start_requested());
    }

    /// End-to-end press path for profile push-to-talk: a synthetic F-key
    /// egui event must engage the matching profile, start capture, and stop
    /// on release. Guards the full parse → match → engine chain that no
    /// smoke lane could exercise live (the mac runner has no input device).
    #[cfg(feature = "speech")]
    #[test]
    fn f_key_events_drive_profile_hold_dictation_end_to_end() {
        use egui::{Event, Key, Modifiers, RawInput};

        let press = |key| Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let release = |key| Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let frame = |events| RawInput {
            events,
            ..RawInput::default()
        };

        let ctx = Context::default();
        let (mut speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1", "F2", "F3"]);
        let target = PanelId(7);
        let mut engaged = None;
        let mut events = Vec::new();

        // A key with no profile binding must not engage anything.
        let _ = ctx.run(frame(vec![press(Key::K)]), |ctx| {
            engaged = handle_profile_hotkeys(
                ctx,
                &mut speech,
                Some(SpeechTarget::Terminal(target)),
                true,
                engaged,
                &mut events,
            );
        });
        assert_eq!(engaged, None);
        assert_eq!(speech.recording_target(), None);

        // A Carbon-registered key's mirrored egui press is not a second
        // activation source while the global path owns it.
        let _ = ctx.run(frame(vec![press(Key::F1)]), |ctx| {
            engaged = handle_profile_hotkeys(
                ctx,
                &mut speech,
                Some(SpeechTarget::Terminal(target)),
                false,
                engaged,
                &mut events,
            );
        });
        assert_eq!(engaged, None);
        assert_eq!(speech.recording_target(), None);
        assert!(!channels.capture_start_requested());

        // F2 engages the second profile and starts capture into the target.
        let _ = ctx.run(frame(vec![press(Key::F2)]), |ctx| {
            engaged = handle_profile_hotkeys(
                ctx,
                &mut speech,
                Some(SpeechTarget::Terminal(target)),
                true,
                engaged,
                &mut events,
            );
        });
        assert_eq!(engaged, Some(1));
        assert_eq!(speech.recording_target(), Some(SpeechTarget::Terminal(target)));
        assert!(channels.capture_start_requested());

        // Releasing the engaged key stops the hold (recording ends, the
        // engine moves on to awaiting the captured PCM).
        let _ = ctx.run(frame(vec![release(Key::F2)]), |ctx| {
            engaged = handle_profile_hotkeys(
                ctx,
                &mut speech,
                Some(SpeechTarget::Terminal(target)),
                true,
                engaged,
                &mut events,
            );
        });
        assert_eq!(engaged, None);
        assert_eq!(speech.recording_target(), None);
        assert!(speech.is_active());
        // Unbound keys, a clean start, and a clean stop must not produce
        // ignored-press notices.
        assert!(events.is_empty());
    }

    #[test]
    fn hold_hotkey_claims_only_an_idle_session_with_a_focused_terminal() {
        let focused = SpeechTarget::Terminal(PanelId(7));
        let starts = HoldHotkeyTransition {
            start_target: Some(focused),
            stop: false,
            engaged_profile: Some(1),
        };
        assert_eq!(
            hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, Some(focused)),
            starts
        );

        let ignored = HoldHotkeyTransition {
            start_target: None,
            stop: false,
            engaged_profile: None,
        };
        assert_eq!(
            hold_hotkey_transition(1, true, false, None, SpeechActivity::Recording, Some(focused)),
            ignored
        );
        assert_eq!(
            hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, None),
            ignored
        );
    }

    #[test]
    fn hold_hotkey_same_batch_tap_stops_only_its_own_recording() {
        let focused = SpeechTarget::Terminal(PanelId(7));
        assert_eq!(
            hold_hotkey_transition(1, true, true, None, SpeechActivity::Idle, Some(focused)),
            HoldHotkeyTransition {
                start_target: Some(focused),
                stop: true,
                engaged_profile: None,
            }
        );

        let mic_button_press = hold_hotkey_transition(1, true, false, None, SpeechActivity::Recording, Some(focused));
        assert_eq!(mic_button_press.engaged_profile, None);
        assert!(!hold_hotkey_transition(1, false, true, None, SpeechActivity::Recording, Some(focused)).stop);
        assert!(hold_hotkey_transition(1, false, true, Some(1), SpeechActivity::Recording, Some(focused)).stop);
        assert!(!hold_hotkey_transition(2, false, true, Some(1), SpeechActivity::Recording, Some(focused)).stop);
    }

    #[test]
    fn hold_hotkey_drops_stale_ownership_after_recording_ends() {
        let transition = hold_hotkey_transition(
            1,
            false,
            true,
            Some(1),
            SpeechActivity::Busy,
            Some(SpeechTarget::Terminal(PanelId(7))),
        );
        assert_eq!(transition.engaged_profile, None);
        assert!(!transition.stop);
    }
}
