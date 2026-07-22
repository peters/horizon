use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Config, GitWatcher, PanelId, PanelKind, WorkspaceId};

use super::super::input;
use crate::{loading_spinner, theme};

use super::canvas::CanvasGridCache;
use super::{HorizonApp, WS_BG_PAD, WS_TITLE_HEIGHT, attention_feed};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HoldHotkeyTransition {
    start_target: Option<PanelId>,
    stop: bool,
    engaged_profile: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpeechActivity {
    Idle,
    Recording,
    Busy,
}

fn hold_hotkey_transition(
    profile: usize,
    pressed: bool,
    released: bool,
    engaged_profile: Option<usize>,
    activity: SpeechActivity,
    focused_terminal: Option<PanelId>,
) -> HoldHotkeyTransition {
    let mut engaged_profile = if activity == SpeechActivity::Recording {
        engaged_profile
    } else {
        None
    };
    let start_target = if pressed && engaged_profile.is_none() && activity == SpeechActivity::Idle {
        focused_terminal
    } else {
        None
    };
    if start_target.is_some() {
        engaged_profile = Some(profile);
    }
    let stop = released && engaged_profile == Some(profile);
    if stop {
        engaged_profile = None;
    }
    HoldHotkeyTransition {
        start_target,
        stop,
        engaged_profile,
    }
}

fn speech_activity(speech: &super::speech::SpeechSystem) -> SpeechActivity {
    if speech.recording_target().is_some() {
        SpeechActivity::Recording
    } else if speech.is_active() {
        SpeechActivity::Busy
    } else {
        SpeechActivity::Idle
    }
}

fn handle_profile_hotkeys(
    ctx: &Context,
    speech: &mut super::speech::SpeechSystem,
    focused_terminal: Option<PanelId>,
    presses_allowed: bool,
    mut engaged_profile: Option<usize>,
) -> Option<usize> {
    for index in 0..speech.profile_bindings().len() {
        let (profile, binding) = speech.profile_bindings()[index];
        let (pressed, released) =
            ctx.input(|input| super::shortcuts::press_and_release_in_events(&input.events, binding));
        let pressed = pressed && presses_allowed;
        match speech.hotkey_mode() {
            horizon_core::SpeechHotkeyMode::Hold => {
                let transition = hold_hotkey_transition(
                    profile,
                    pressed,
                    released,
                    engaged_profile,
                    speech_activity(speech),
                    focused_terminal,
                );
                if let Some(focused) = transition.start_target {
                    speech.start(focused, profile);
                }
                if transition.stop {
                    speech.stop();
                }
                engaged_profile = transition.engaged_profile;
            }
            horizon_core::SpeechHotkeyMode::Toggle => {
                if pressed {
                    if speech.recording_target().is_some() {
                        speech.stop();
                    } else if let Some(focused) = focused_terminal {
                        speech.start(focused, profile);
                    }
                }
            }
        }
    }
    engaged_profile
}

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

    /// Push-to-talk hotkey handling plus draining speech results into the
    /// target panel's PTY input (mirrors `poll_primary_selection_paste`).
    ///
    /// The hotkey listens on the root viewport only; panels in detached
    /// windows still dictate via their mic button.
    fn handle_speech_input(&mut self, ctx: &Context) {
        self.speech_escape_cancelled = false;
        // The hotkey targets the focused panel, but only terminal-backed
        // panels can receive typed text.
        let focused_terminal = self.board.focused.filter(|id| {
            self.board.panel(*id).is_some_and(|panel| {
                // The root-viewport hotkey must not dictate into a panel
                // living in a detached window (documented main-window scope).
                panel.terminal().is_some() && !self.workspace_is_detached(panel.workspace_id)
            })
        });
        // Capture-state hygiene must run even without a speech runtime
        // (stub builds, or Speech Input disabled with Rebind still armed):
        // a stale flag would suppress global shortcuts indefinitely.
        let mut capturing_hotkey: bool = ctx
            .data(|data| data.get_temp(egui::Id::new("speech_hotkey_capturing")))
            .unwrap_or(false);
        if capturing_hotkey && !self.settings_speech_tab_open() {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("speech_hotkey_capturing"), false));
            capturing_hotkey = false;
        }
        // A just-captured chord suppresses global shortcuts until its key
        // release is seen; if the window loses focus first, that release may
        // never arrive (Wayland/macOS), so recover the pending key here or it
        // would disable every shortcut indefinitely.
        let root_focused_now = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        if !root_focused_now {
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("speech_captured_key"),
                    None::<super::settings::PendingCapture>,
                );
            });
        }

        if !root_focused_now {
            self.stop_hold_on_focus_loss();
        }

        let Some(speech) = self.speech.as_mut() else {
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            self.reset_speech_input_state(root_focused_now);
            return;
        };

        // Invariant: the active target panel must still exist — across
        // Recording, AwaitingPcm, and Transcribing. Covers every removal
        // path (single close, workspace bulk close, session teardown), so
        // the mic can't stay open and a busy engine can't wedge behind a
        // vanished panel while inference finishes into nothing.
        if let Some(target) = speech.active_target()
            && self.board.panel(target).is_none()
        {
            speech.cancel();
            // Held bindings persist until their release is consumed, so a
            // key-up after the panel vanished cannot leak into the terminal.
            self.speech_engaged_profile = None;
            tracing::info!("recording target panel disappeared; recording cancelled");
        }

        // Seed the per-frame focus aggregate with the root viewport; each
        // detached viewport ORs itself in during rendering, and the privacy
        // guard in `finalize_frame` cancels an unattended recording when no
        // Horizon window has focus (see `cancel_unattended_recording`).
        let root_focused = root_focused_now;
        self.any_viewport_focused = root_focused;

        // While the settings binder is capturing a new hotkey, the pressed
        // chord must not also trigger the current binding. And while a
        // text-entry surface is open (settings, command palette, search,
        // rename), a printable hotkey belongs to that surface — engaging
        // would both type and record. Terminal focus is NOT such a surface:
        // dictating into the focused terminal is the normal case. Only new
        // presses are gated; releases below always run so a hold started
        // before opening a surface still stops.
        let text_surface_active = self.settings.is_some()
            || self.command_palette.is_some()
            || self.search_overlay.is_some()
            || self.renaming_panel.is_some()
            || self.renaming_workspace.is_some();
        self.speech_engaged_profile = handle_profile_hotkeys(
            ctx,
            speech,
            focused_terminal,
            !capturing_hotkey && !text_surface_active,
            self.speech_engaged_profile,
        );

        if speech.recording_target().is_some() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            speech.cancel();
            self.speech_engaged_profile = None;
            // Consume the Escape: fullscreen exit and the terminal must not
            // also react to a keypress that meant "cancel dictation".
            self.speech_escape_cancelled = true;
        }
        if speech.is_active() {
            // Keep frames coming so the pulse animates and poll() runs
            // promptly even when the terminal is otherwise idle, but bounded
            // so a long transcription doesn't spin the render loop.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Publish the selected backend for the settings UI (relevant when
        // the config says `auto`). Clear it while unknown so a stale value
        // from before a live config rebuild is not displayed.
        match speech.active_backend() {
            Some(backend) => {
                let backend = backend.to_string();
                ctx.data_mut(|data| data.insert_temp(egui::Id::new("speech_active_backend"), backend));
            }
            None => {
                ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            }
        }

        let events = speech.poll();
        // Capture/worker failures can end Recording during poll(), after the
        // transition above ran. Drop ownership before rendering can start a
        // new mic-button recording in this same frame.
        if speech.recording_target().is_none() {
            self.speech_engaged_profile = None;
        }
        self.inject_speech_events(events);
    }

    /// Deliver transcripts into their target panels (mirrors
    /// `poll_primary_selection_paste`); errors are logged.
    fn inject_speech_events(&mut self, events: Vec<super::speech::SpeechEvent>) {
        for event in events {
            match event {
                super::speech::SpeechEvent::Text { target, text } => {
                    let Some(panel) = self.board.panel_mut(target) else {
                        tracing::warn!("speech target panel closed before transcription finished");
                        continue;
                    };
                    let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
                        continue;
                    };
                    // Trailing space so consecutive dictations don't fuse words.
                    let bytes = input::paste_bytes(&format!("{text} "), mode, true);
                    panel.write_input(&bytes);
                }
                super::speech::SpeechEvent::Error(message) => {
                    tracing::warn!(%message, "speech input error");
                }
            }
        }
    }

    /// Privacy guard: on Wayland and macOS, winit synthesizes no key release
    /// when a window loses focus mid-hold, so the release that would stop a
    /// recording never arrives — and a recording in a detached window gets no
    /// root-viewport event at all. Evaluated after every viewport rendered:
    /// if no Horizon window has focus, the microphone must not stay open.
    fn cancel_unattended_recording(&mut self) {
        if self.any_viewport_focused {
            return;
        }
        if let Some(speech) = self.speech.as_mut()
            && speech.recording_target().is_some()
        {
            speech.cancel();
            self.speech_engaged_profile = None;
            // Focus left every Horizon window: the pending releases will not
            // reach our terminals, so clearing avoids a stuck-swallow after
            // focus returns.
            self.speech_held_bindings.clear();
            self.speech_escape_release_pending = false;
            tracing::info!("all Horizon windows lost focus during dictation; recording cancelled");
        }
    }

    /// Hold-mode release detection is root-only, but the release can land in
    /// a detached Horizon window when focus moved there mid-hold (and on
    /// Wayland/macOS focus loss synthesizes no key release at all). Treat the
    /// focus loss as the release, and drop the held chords: their key-up will
    /// land in whatever took focus, so the root filter will never consume it.
    fn stop_hold_on_focus_loss(&mut self) {
        // Retiring the filter state is unconditional. The chord's key-up
        // lands wherever focus went, so the root filter will never consume
        // it — whether or not the press started a recording. A press that
        // was a no-op still registers as held, because the filter does not
        // know whether the engine acted on it: that happens when the focused
        // panel is not a terminal, when the engine is still transcribing, or
        // when a recording is already running. Gating this clear on an
        // engaged profile strands such an entry, and it then swallows that
        // key, its repeats, and its text from the next terminal typed into.
        self.speech_held_bindings.clear();
        self.speech_escape_release_pending = false;
        let Some(speech) = self.speech.as_mut() else {
            return;
        };
        if self.speech_engaged_profile.is_some() && speech.hotkey_mode() == horizon_core::SpeechHotkeyMode::Hold {
            speech.stop();
            self.speech_engaged_profile = None;
        }
    }

    /// Seed the focus aggregate and drop stale held-chord state when speech
    /// is unavailable: an undelivered key-up (focus lost while disabled)
    /// must not keep swallowing that key forever.
    fn reset_speech_input_state(&mut self, root_focused: bool) {
        self.any_viewport_focused = root_focused;
        if !root_focused {
            self.speech_held_bindings.clear();
            self.speech_engaged_profile = None;
            self.speech_escape_release_pending = false;
        }
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

    use eframe::CreationContext;
    use egui::Context;
    use horizon_core::{Config, HorizonHome, PanelId, RuntimeState, SessionStore, StartupDecision};

    use super::{HoldHotkeyTransition, HorizonApp, SpeechActivity, hold_hotkey_transition};
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

    /// A push-to-talk press that started no recording still registers with
    /// the terminal filter, so focus loss must retire it: its key-up goes to
    /// whatever took focus, and a stranded entry swallows that key — and the
    /// text it produces — from the next terminal the user types into.
    #[test]
    fn focus_loss_retires_a_held_chord_that_never_started_a_recording() {
        let mut app = test_app();
        let chord = horizon_core::ShortcutBinding::new(
            horizon_core::ShortcutModifiers::CTRL,
            horizon_core::ShortcutKey::Letter('K'),
        );
        // Pressed with no terminal focused: the filter holds the chord, but
        // the engine never engaged a profile.
        app.speech_held_bindings.push(chord);
        app.speech_engaged_profile = None;
        app.speech_escape_release_pending = true;

        app.stop_hold_on_focus_loss();

        assert!(
            app.speech_held_bindings.is_empty(),
            "a no-op press must not stay held once focus left the root window"
        );
        assert!(!app.speech_escape_release_pending);
    }

    #[test]
    fn hold_hotkey_claims_only_an_idle_session_with_a_focused_terminal() {
        let focused = PanelId(7);
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
        let focused = PanelId(7);
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
        let transition = hold_hotkey_transition(1, false, true, Some(1), SpeechActivity::Busy, Some(PanelId(7)));
        assert_eq!(transition.engaged_profile, None);
        assert!(!transition.stop);
    }
}
