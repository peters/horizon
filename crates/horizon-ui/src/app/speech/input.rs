//! Frame-loop speech glue: push-to-talk, result injection, and notices.
//!
//! Split from `lifecycle.rs` so that module stays under the line cap and
//! speech input has one home. Behavior is unchanged.

use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{PanelId, WorkspaceId};

use super::super::shortcuts;
use super::super::{HorizonApp, SpeechNotice};
use super::desktop::{dictation_sink, inject_desktop_transcript, recv_global_hotkey};
use super::{SpeechEvent, SpeechSink, SpeechSystem};
use crate::input;
use crate::theme;

pub(crate) const SPEECH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SPEECH_RELEASE_OWNERSHIP_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HoldHotkeyTransition {
    start_target: Option<SpeechSink>,
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
    sink: Option<SpeechSink>,
) -> HoldHotkeyTransition {
    let mut engaged_profile = if activity == SpeechActivity::Recording {
        engaged_profile
    } else {
        None
    };
    let start_target = if pressed && engaged_profile.is_none() && activity == SpeechActivity::Idle {
        sink
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

fn speech_activity(speech: &SpeechSystem) -> SpeechActivity {
    if speech.recording_sink().is_some() {
        SpeechActivity::Recording
    } else if speech.is_active() {
        SpeechActivity::Busy
    } else {
        SpeechActivity::Idle
    }
}

/// Why a matched push-to-talk press could not start a recording. Every
/// no-op press produces a notice: a press that visibly does nothing is
/// indistinguishable from a dead hotkey, which is exactly how the silent
/// variants were reported.
fn no_start_notice(activity: SpeechActivity) -> SpeechEvent {
    // Mode-neutral wording: these fire for hold and toggle hotkeys alike,
    // and the same activity states are reachable from the mic button.
    let message = match activity {
        SpeechActivity::Recording => "Another dictation is already recording — stop it first.",
        SpeechActivity::Busy => {
            "Hotkey ignored — still processing the previous dictation (the first use also loads the model, which can take a while)."
        }
        SpeechActivity::Idle => "Focus a terminal panel to dictate into it.",
    };
    SpeechEvent::Notice(message.to_string())
}

/// Which text surface ate a gated push-to-talk press, for the notice. The
/// priority order mirrors `text_surface_active`; rename fields are the
/// remaining case.
fn clear_stale_hotkey_capture(ctx: &Context, settings_speech_tab_open: bool) -> bool {
    let mut capturing_hotkey: bool = ctx
        .data(|data| data.get_temp(egui::Id::new("speech_hotkey_capturing")))
        .unwrap_or(false);
    if capturing_hotkey && !settings_speech_tab_open {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("speech_hotkey_capturing"), false));
        capturing_hotkey = false;
    }
    capturing_hotkey
}

fn terminal_matches_focused_viewport(
    panel_workspace: WorkspaceId,
    panel_is_detached: bool,
    root_focused: bool,
    focused_detached: Option<WorkspaceId>,
) -> bool {
    if let Some(detached) = focused_detached {
        return panel_workspace == detached;
    }
    root_focused && !panel_is_detached
}

fn gated_press_surface(settings_open: bool, palette_open: bool, search_capturing: bool) -> &'static str {
    if settings_open {
        "the settings window is open"
    } else if palette_open {
        "the command palette is open"
    } else if search_capturing {
        "the search field has focus"
    } else {
        "a rename field is open"
    }
}

fn handle_profile_hotkeys(
    ctx: &Context,
    speech: &mut SpeechSystem,
    sink: Option<SpeechSink>,
    presses_allowed: bool,
    mut engaged_profile: Option<usize>,
    events: &mut Vec<SpeechEvent>,
    skip_profiles: &[usize],
) -> Option<usize> {
    for index in 0..speech.profile_bindings().len() {
        let (profile, binding) = speech.profile_bindings()[index];
        if skip_profiles.contains(&profile) {
            continue;
        }
        let (pressed, released) = ctx.input(|input| shortcuts::press_and_release_in_events(&input.events, binding));
        let pressed = pressed && presses_allowed;
        if pressed {
            tracing::info!(
                profile,
                activity = ?speech_activity(speech),
                desktop = matches!(sink, Some(SpeechSink::Desktop)),
                "speech hotkey pressed"
            );
        }
        match speech.hotkey_mode() {
            horizon_core::SpeechHotkeyMode::Hold => {
                let transition = hold_hotkey_transition(
                    profile,
                    pressed,
                    released,
                    engaged_profile,
                    speech_activity(speech),
                    sink,
                );
                if let Some(target) = transition.start_target {
                    speech.start(target, profile);
                } else if pressed {
                    events.push(no_start_notice(speech_activity(speech)));
                }
                if transition.stop {
                    speech.stop();
                }
                engaged_profile = transition.engaged_profile;
            }
            horizon_core::SpeechHotkeyMode::Toggle => {
                if pressed {
                    if speech.recording_sink().is_some() {
                        speech.stop();
                    } else if speech_activity(speech) != SpeechActivity::Idle {
                        // `start` below no-ops outside Idle; explain instead.
                        events.push(no_start_notice(speech_activity(speech)));
                    } else if let Some(target) = sink {
                        speech.start(target, profile);
                    } else {
                        events.push(no_start_notice(SpeechActivity::Idle));
                    }
                }
            }
        }
    }
    engaged_profile
}

fn apply_global_hotkey_events(
    speech: &mut SpeechSystem,
    events: impl IntoIterator<Item = horizon_cursor::HotkeyEvent>,
    sink: Option<SpeechSink>,
    presses_allowed: bool,
    mut engaged_profile: Option<usize>,
    notices: &mut Vec<SpeechEvent>,
) -> (Option<usize>, Vec<horizon_cursor::HotkeyEvent>, bool) {
    let mut deferred = Vec::new();
    let mut started_this_drain = false;
    let mut disconnected = false;
    for event in events {
        if event == horizon_cursor::HotkeyEvent::Disconnected {
            disconnected = true;
            if speech.recording_sink().is_some() {
                speech.cancel();
            }
            engaged_profile = None;
            continue;
        }
        if started_this_drain {
            deferred.push(event);
            continue;
        }
        match event {
            horizon_cursor::HotkeyEvent::Pressed(profile) => {
                tracing::info!(
                    profile,
                    activity = ?speech_activity(speech),
                    desktop = matches!(sink, Some(SpeechSink::Desktop)),
                    "speech global hotkey pressed"
                );
                if !presses_allowed {
                    notices.push(no_start_notice(speech_activity(speech)));
                    continue;
                }
                match speech.hotkey_mode() {
                    horizon_core::SpeechHotkeyMode::Hold => {
                        let transition = hold_hotkey_transition(
                            profile,
                            true,
                            false,
                            engaged_profile,
                            speech_activity(speech),
                            sink,
                        );
                        if let Some(target) = transition.start_target {
                            speech.start(target, profile);
                            started_this_drain = true;
                        } else {
                            notices.push(no_start_notice(speech_activity(speech)));
                        }
                        engaged_profile = transition.engaged_profile;
                    }
                    horizon_core::SpeechHotkeyMode::Toggle => {
                        if speech.recording_sink().is_some() {
                            speech.stop();
                        } else if speech_activity(speech) != SpeechActivity::Idle {
                            notices.push(no_start_notice(speech_activity(speech)));
                        } else if let Some(target) = sink {
                            speech.start(target, profile);
                            engaged_profile = Some(profile);
                            started_this_drain = true;
                        } else {
                            notices.push(no_start_notice(SpeechActivity::Idle));
                        }
                    }
                }
            }
            horizon_cursor::HotkeyEvent::Released(profile) => {
                if speech.hotkey_mode() == horizon_core::SpeechHotkeyMode::Hold && engaged_profile == Some(profile) {
                    speech.stop();
                    engaged_profile = None;
                }
            }
            horizon_cursor::HotkeyEvent::Disconnected => {}
        }
    }
    (engaged_profile, deferred, disconnected)
}

fn apply_global_hotkeys(
    speech: &mut SpeechSystem,
    hotkeys: Option<&horizon_cursor::GlobalHotkeys>,
    pending: &mut Vec<horizon_cursor::HotkeyEvent>,
    sink: Option<SpeechSink>,
    presses_allowed: bool,
    engaged_profile: Option<usize>,
    events: &mut Vec<SpeechEvent>,
) -> (Option<usize>, bool) {
    let mut incoming = std::mem::take(pending);
    while let Some(event) = recv_global_hotkey(hotkeys) {
        incoming.push(event);
    }
    let (engaged_profile, deferred, disconnected) =
        apply_global_hotkey_events(speech, incoming, sink, presses_allowed, engaged_profile, events);
    *pending = deferred;
    (engaged_profile, disconnected)
}

impl HorizonApp {
    pub(in crate::app) fn cancel_speech_target(&mut self, panel_id: PanelId) -> bool {
        let Some(speech) = self.speech.as_mut() else {
            return false;
        };
        if speech.active_target() != Some(panel_id) {
            return false;
        }

        speech.cancel();
        // Held bindings remain owned until their physical release is consumed,
        // so the release cannot leak into either the old or replacement PTY.
        self.speech_engaged_profile = None;
        true
    }

    /// Push-to-talk hotkey handling plus draining speech results into the
    /// target panel's PTY input (mirrors `poll_primary_selection_paste`).
    fn focused_horizon_terminal(&self, ctx: &Context, root_focused: bool) -> Option<PanelId> {
        let panel_id = self.board.focused?;
        let panel = self.board.panel(panel_id)?;
        panel.terminal()?;
        terminal_matches_focused_viewport(
            panel.workspace_id,
            self.workspace_is_detached(panel.workspace_id),
            root_focused,
            self.focused_detached_workspace_id(ctx),
        )
        .then_some(panel_id)
    }

    fn speech_text_surface_active(&self) -> (bool, bool) {
        let search_capturing = self
            .search_overlay
            .as_ref()
            .is_some_and(crate::search_overlay::SearchOverlay::input_focused);
        let text_surface_active = self.settings.is_some()
            || self.command_palette.is_some()
            || search_capturing
            || self.renaming_panel.is_some()
            || self.renaming_workspace.is_some();
        (text_surface_active, search_capturing)
    }

    fn cancel_vanished_speech_target(&mut self) {
        let vanished_target = self
            .speech
            .as_ref()
            .and_then(SpeechSystem::active_target)
            .filter(|target| self.board.panel(*target).is_none());
        if let Some(target) = vanished_target {
            let _ = self.cancel_speech_target(target);
            tracing::info!("recording target panel disappeared; recording cancelled");
        }
    }

    fn install_speech_global_wake(&self, ctx: &Context) {
        if let Some(hotkeys) = self.speech_global_hotkeys.as_ref()
            && !hotkeys.has_wake()
        {
            let ctx = ctx.clone();
            hotkeys.set_wake(move || ctx.request_repaint());
        }
    }

    pub(in crate::app) fn handle_speech_input(&mut self, ctx: &Context) {
        let now = Instant::now();
        self.expire_speech_release_ownership(now);
        self.speech_escape_cancelled = false;
        let desktop_injection = self.template_config.features.speech.desktop_injection;
        // Capture-state hygiene must run even without a speech runtime
        // (stub builds, or Speech Input disabled with Rebind still armed):
        // a stale flag would suppress global shortcuts indefinitely.
        let capturing_hotkey = clear_stale_hotkey_capture(ctx, self.settings_speech_tab_open());
        // A just-captured chord suppresses global shortcuts until its key
        // release is seen; if the window loses focus first, that release may
        // never arrive (Wayland/macOS), so recover the pending key here or it
        // would disable every shortcut indefinitely.
        let root_focused_now = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        let horizon_focused = root_focused_now || self.any_detached_viewport_focused(ctx);
        let (text_surface_active, search_capturing) = self.speech_text_surface_active();
        self.sync_speech_global_hotkeys_for_surfaces(capturing_hotkey, text_surface_active, horizon_focused);
        self.install_speech_global_wake(ctx);
        let sink = dictation_sink(
            self.focused_horizon_terminal(ctx, root_focused_now),
            desktop_injection,
            horizon_focused,
        );
        if !root_focused_now {
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("speech_captured_key"),
                    None::<super::super::settings::PendingCapture>,
                );
            });
        }

        if !horizon_focused {
            self.stop_hold_on_focus_loss(ctx, now);
        }

        if self.speech.is_none() {
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            self.reset_speech_input_state(horizon_focused);
            return;
        }

        self.cancel_vanished_speech_target();

        let Some(speech) = self.speech.as_mut() else {
            return;
        };

        // Seed the per-frame focus aggregate from every Horizon window so
        // global PTT in a detached viewport is not treated as desktop
        // dictation. Detached viewports still OR themselves in during
        // rendering; `cancel_unattended_recording` consumes the result.
        self.any_viewport_focused = horizon_focused;

        let mut events = Vec::new();
        // A push-to-talk chord pressed while its presses are gated must not
        // read as a dead hotkey — say exactly which surface is eating the
        // press, so a stale gate state can never hide again. The binder's
        // own capture is exempt: there the press is recorded, not ignored.
        if text_surface_active && !capturing_hotkey {
            let gated_press = ctx.input(|input| {
                speech
                    .profile_bindings()
                    .iter()
                    .any(|(_, binding)| shortcuts::press_and_release_in_events(&input.events, *binding).0)
            });
            if gated_press {
                let surface = gated_press_surface(
                    self.settings.is_some(),
                    self.command_palette.is_some(),
                    search_capturing,
                );
                events.push(SpeechEvent::Notice(format!("Push-to-talk press ignored: {surface}.")));
            }
        }
        let grabbed = self
            .speech_global_hotkeys
            .as_ref()
            .map_or(Vec::new(), |hotkeys| hotkeys.profiles().to_vec());
        let mut engaged = self.speech_engaged_profile;
        if self.speech_global_hotkeys.is_some() {
            let presses_allowed = !(capturing_hotkey || text_surface_active && horizon_focused);
            let disconnected;
            (engaged, disconnected) = apply_global_hotkeys(
                speech,
                self.speech_global_hotkeys.as_ref(),
                &mut self.speech_global_events_pending,
                sink,
                presses_allowed,
                engaged,
                &mut events,
            );
            if disconnected {
                tracing::warn!("desktop dictation: global hotkey listener disconnected");
                engaged = None;
                self.speech_global_hotkeys = None;
                self.speech_global_hotkeys_tried = false;
                self.speech_global_hotkeys_suspended = false;
                self.speech_global_events_pending.clear();
            }
            if !self.speech_global_events_pending.is_empty() {
                ctx.request_repaint();
            }
        }
        self.speech_engaged_profile = handle_profile_hotkeys(
            ctx,
            speech,
            sink,
            !capturing_hotkey && !text_surface_active,
            engaged,
            &mut events,
            &grabbed,
        );

        // Escape cancels every active speech state, not just Recording:
        // AwaitingPcm and Transcribing cover the multi-second first-use
        // model load, exactly when a user most wants to abort — and
        // `SpeechSystem::cancel` supports all of them.
        if speech.is_active() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
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
            ctx.request_repaint_after(SPEECH_POLL_INTERVAL);
        }

        self.poll_speech_runtime(ctx, events);
    }

    /// Drain speech worker events even while the engine is otherwise idle.
    /// Startup preloads complete in `State::Idle`, so they need their own
    /// bounded repaint loop and must be polled from views that return before
    /// normal input processing.
    pub(in crate::app) fn poll_speech_runtime(&mut self, ctx: &Context, mut events: Vec<SpeechEvent>) {
        let Some(speech) = self.speech.as_mut() else {
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            self.inject_speech_events(events);
            return;
        };

        events.extend(speech.poll());
        if speech.has_pending_preloads() {
            ctx.request_repaint_after(SPEECH_POLL_INTERVAL);
        }
        // Publish after polling so a preload success is visible to settings in
        // the same frame that drains its worker event.
        let active_backend = speech.active_backend().map(str::to_owned);
        // Capture/worker failures can end Recording during poll(), after the
        // transition above ran. Drop ownership before rendering can start a
        // new mic-button recording in this same frame.
        let recording_finished = speech.recording_sink().is_none();

        match active_backend {
            Some(backend) => {
                ctx.data_mut(|data| data.insert_temp(egui::Id::new("speech_active_backend"), backend));
            }
            None => {
                ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            }
        }
        if recording_finished {
            self.speech_engaged_profile = None;
        }
        self.inject_speech_events(events);
    }

    /// Deliver transcripts into their target panels (mirrors
    /// `poll_primary_selection_paste`); errors are logged.
    fn inject_speech_events(&mut self, events: Vec<SpeechEvent>) {
        for event in events {
            match event {
                SpeechEvent::Text { target, text } => match target {
                    SpeechSink::Panel(panel_id) => {
                        let Some(panel) = self.board.panel_mut(panel_id) else {
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
                    SpeechSink::Desktop => {
                        let payload = format!("{text} ");
                        match inject_desktop_transcript(&payload) {
                            Ok(()) => {}
                            Err(horizon_cursor::InjectError::Unsupported) => {
                                self.show_speech_notice("Transcript copied — paste with Ctrl+V", false);
                            }
                            Err(horizon_cursor::InjectError::Clipboard(message)) => {
                                tracing::warn!(%message, "desktop speech clipboard failed");
                                self.show_speech_notice(
                                    format!("Speech input error: could not copy transcript ({message})"),
                                    true,
                                );
                            }
                            Err(error) => {
                                tracing::warn!(%error, "desktop speech inject failed");
                                self.show_speech_notice(
                                    format!("Transcript copied — paste with Ctrl+V ({error})"),
                                    true,
                                );
                            }
                        }
                    }
                },
                SpeechEvent::Notice(message) => {
                    tracing::info!(%message, "speech notice");
                    self.show_speech_notice(message, false);
                }
                SpeechEvent::Error(message) => {
                    tracing::warn!(%message, "speech input error");
                    self.show_speech_notice(format!("Speech input error: {message}"), true);
                }
            }
        }
    }

    pub(in crate::app) fn show_speech_notice(&mut self, message: impl Into<String>, error: bool) {
        self.speech_notice = Some(SpeechNotice {
            message: message.into(),
            error,
            shown_at: Instant::now(),
        });
    }

    /// Bottom-center transient feedback for dictation outcomes that would
    /// otherwise be invisible (ignored presses, empty transcripts, errors).
    pub(in crate::app) fn render_speech_notice(&mut self, ctx: &Context) {
        const NOTICE_TTL: Duration = Duration::from_secs(5);
        const NOTICE_MAX_WIDTH: f32 = 720.0;
        const NOTICE_VIEWPORT_MARGIN: f32 = 48.0;
        let Some(notice) = &self.speech_notice else {
            return;
        };
        let elapsed = notice.shown_at.elapsed();
        let Some(remaining) = NOTICE_TTL.checked_sub(elapsed).filter(|left| !left.is_zero()) else {
            self.speech_notice = None;
            return;
        };
        ctx.request_repaint_after(remaining);
        let (icon, tint) = if notice.error {
            ("⚠", theme::PALETTE_YELLOW())
        } else {
            ("🎤", theme::FG())
        };
        let max_width = (ctx.content_rect().width() - NOTICE_VIEWPORT_MARGIN).clamp(1.0, NOTICE_MAX_WIDTH);
        egui::Area::new(egui::Id::new("speech_notice_overlay"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(max_width);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(egui::RichText::new(icon).color(tint).size(12.0));
                        ui.add(
                            egui::Label::new(egui::RichText::new(&notice.message).color(theme::FG()).size(12.0)).wrap(),
                        );
                    });
                });
            });
    }

    /// Privacy guard: on Wayland and macOS, winit synthesizes no key release
    /// when a window loses focus mid-hold, so the release that would stop a
    /// recording never arrives — and a recording in a detached window gets no
    /// root-viewport event at all. Evaluated after every viewport rendered:
    /// if no Horizon window has focus, the microphone must not stay open.
    pub(in crate::app) fn cancel_unattended_recording(&mut self) {
        if self.any_viewport_focused
            || self.speech.as_ref().and_then(SpeechSystem::recording_sink) == Some(SpeechSink::Desktop)
        {
            return;
        }
        self.speech_engaged_profile = None;
        if let Some(speech) = self.speech.as_mut()
            && speech.recording_sink().is_some()
        {
            speech.cancel();
            tracing::info!("all Horizon windows lost focus during dictation; recording cancelled");
        }
    }

    /// Hold-mode release detection is root-only, but the release can land in
    /// a detached Horizon window when focus moved there mid-hold (and on
    /// Wayland/macOS focus loss synthesizes no key release at all). Treat root
    /// focus loss as the recording release, but retain terminal-filter state:
    /// a detached viewport must still consume the physical key-up.
    pub(in crate::app) fn stop_hold_on_focus_loss(&mut self, ctx: &Context, now: Instant) {
        if self.speech.as_ref().and_then(SpeechSystem::recording_sink) == Some(SpeechSink::Desktop) {
            return;
        }
        self.arm_speech_release_ownership(ctx, now);
        let Some(speech) = self.speech.as_mut() else {
            return;
        };
        if self.speech_engaged_profile.is_some() && speech.hotkey_mode() == horizon_core::SpeechHotkeyMode::Hold {
            speech.stop();
            self.speech_engaged_profile = None;
        }
    }

    pub(in crate::app) fn arm_speech_release_ownership(&mut self, ctx: &Context, now: Instant) {
        let deadline = now + SPEECH_RELEASE_OWNERSHIP_TIMEOUT;
        let mut armed = false;
        for held in &mut self.speech_held_bindings {
            if held.release_deadline.is_none() {
                held.release_deadline = Some(deadline);
                armed = true;
            }
        }
        if self.speech_escape_release_pending && self.speech_escape_release_deadline.is_none() {
            self.speech_escape_release_deadline = Some(deadline);
            armed = true;
        }
        if armed {
            ctx.request_repaint_after(SPEECH_RELEASE_OWNERSHIP_TIMEOUT);
        }
    }

    fn expire_speech_release_ownership(&mut self, now: Instant) {
        self.speech_held_bindings
            .retain(|held| held.release_deadline.is_none_or(|deadline| now < deadline));
        if self
            .speech_escape_release_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.speech_escape_release_pending = false;
            self.speech_escape_release_deadline = None;
        }
    }

    /// Seed the focus aggregate when speech is unavailable. Held-chord state
    /// remains until a viewport consumes the release or its bounded ownership
    /// window expires.
    fn reset_speech_input_state(&mut self, root_focused: bool) {
        self.any_viewport_focused = root_focused;
        if !root_focused {
            self.speech_engaged_profile = None;
        }
    }
}

#[cfg(test)]
mod tests;
