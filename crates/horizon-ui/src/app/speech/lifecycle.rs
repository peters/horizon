//! Main-thread speech integration: local/global hotkey routing, target
//! lifetime checks, worker polling, and transcript delivery.

use std::time::{Duration, Instant};

use egui::Context;

use super::external_text::FocusedTarget;
use super::{SpeechEvent, SpeechTarget};
use crate::app::HorizonApp;
use crate::{input, theme};

pub(in crate::app) const SPEECH_RELEASE_OWNERSHIP_TIMEOUT: Duration = Duration::from_secs(3);
pub(in crate::app) const SPEECH_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) struct HoldHotkeyTransition {
    pub(in crate::app) start_target: Option<SpeechTarget>,
    pub(in crate::app) stop: bool,
    pub(in crate::app) engaged_profile: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::app) enum SpeechActivity {
    Idle,
    Recording,
    Busy,
}

pub(in crate::app) fn hold_hotkey_transition(
    profile: usize,
    pressed: bool,
    released: bool,
    engaged_profile: Option<usize>,
    activity: SpeechActivity,
    target: Option<SpeechTarget>,
) -> HoldHotkeyTransition {
    let mut engaged_profile = if activity == SpeechActivity::Recording {
        engaged_profile
    } else {
        None
    };
    let start_target = if pressed && engaged_profile.is_none() && activity == SpeechActivity::Idle {
        target
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

fn speech_activity(speech: &super::SpeechSystem) -> SpeechActivity {
    if speech.recording_target().is_some() {
        SpeechActivity::Recording
    } else if speech.is_active() {
        SpeechActivity::Busy
    } else {
        SpeechActivity::Idle
    }
}

fn no_start_notice(activity: SpeechActivity) -> SpeechEvent {
    let message = match activity {
        SpeechActivity::Recording => "Another dictation is already recording — stop it first.",
        SpeechActivity::Busy => {
            "Hotkey ignored — still processing the previous dictation (the first use also loads the model, which can take a while)."
        }
        SpeechActivity::Idle => "Focus a terminal panel or editable text field to dictate into it.",
    };
    SpeechEvent::Notice(message.to_string())
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

pub(in crate::app) fn handle_profile_hotkeys(
    ctx: &Context,
    speech: &mut super::SpeechSystem,
    focused_terminal: Option<SpeechTarget>,
    presses_allowed: bool,
    mut engaged_profile: Option<usize>,
    events: &mut Vec<SpeechEvent>,
) -> Option<usize> {
    for index in 0..speech.profile_bindings().len() {
        let (profile, binding) = speech.profile_bindings()[index];
        let (pressed, released) =
            ctx.input(|input| crate::app::shortcuts::press_and_release_in_events(&input.events, binding));
        let pressed = pressed && presses_allowed;
        if pressed {
            tracing::info!(
                profile,
                activity = ?speech_activity(speech),
                terminal_focused = focused_terminal.is_some(),
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
                    focused_terminal,
                );
                if let Some(target) = transition.start_target {
                    let _ = speech.start(target, profile);
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
                    if speech.recording_target().is_some() {
                        speech.stop();
                    } else if speech_activity(speech) != SpeechActivity::Idle {
                        events.push(no_start_notice(speech_activity(speech)));
                    } else if let Some(target) = focused_terminal {
                        let _ = speech.start(target, profile);
                    } else {
                        events.push(no_start_notice(SpeechActivity::Idle));
                    }
                }
            }
        }
    }
    engaged_profile
}

impl HorizonApp {
    /// Handle Horizon-local hotkeys. Fully registered macOS globals are the
    /// activation source even while Horizon is foreground, preventing a
    /// Carbon event and its mirrored egui event from starting twice.
    pub(in crate::app) fn handle_speech_input(&mut self, ctx: &Context) {
        let now = Instant::now();
        self.expire_speech_release_ownership(now);
        self.speech_escape_cancelled = false;
        let focused_terminal = self.focused_terminal_speech_target();

        // Capture-state hygiene must run even without a speech runtime.
        let mut capturing_hotkey: bool = ctx
            .data(|data| data.get_temp(egui::Id::new("speech_hotkey_capturing")))
            .unwrap_or(false);
        if capturing_hotkey && !self.settings_speech_tab_open() {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("speech_hotkey_capturing"), false));
            capturing_hotkey = false;
        }
        // A Settings close/tab switch must not re-register Carbon keys while
        // the captured physical key is still down. The pending-release state
        // outlives the editor and keeps the global manager paused until the
        // release (or the existing bounded timeout) is consumed.
        self.sync_speech_hotkey_rebinding(ctx);
        let root_focused_now = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        if !root_focused_now {
            self.stop_hold_on_focus_loss(ctx, now);
        }

        if self.speech.is_none() {
            self.reset_speech_input_state(root_focused_now);
            // A capture/worker startup failure can leave the runtime absent
            // while a committed global manager still has queued native
            // events. Drain them and clear any retained target instead of
            // allowing a later rebuild to observe stale ownership.
            self.poll_speech_runtime(ctx, Vec::new());
            return;
        }

        self.any_viewport_focused = root_focused_now;
        let search_capturing = self
            .search_overlay
            .as_ref()
            .is_some_and(crate::search_overlay::SearchOverlay::input_focused);
        let text_surface_active = self.text_surface_active(search_capturing);
        let globals_registered = self.speech_global_hotkeys.is_registered();
        let mut events = Vec::new();

        if text_surface_active && !capturing_hotkey && !globals_registered {
            let gated_press = ctx.input(|input| {
                self.speech.as_ref().is_some_and(|speech| {
                    speech.profile_bindings().iter().any(|(_, binding)| {
                        crate::app::shortcuts::press_and_release_in_events(&input.events, *binding).0
                    })
                })
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

        let presses_allowed = !globals_registered && !capturing_hotkey && !text_surface_active;
        if let Some(speech) = self.speech.as_mut() {
            self.speech_engaged_profile = handle_profile_hotkeys(
                ctx,
                speech,
                focused_terminal,
                presses_allowed,
                self.speech_engaged_profile,
                &mut events,
            );
        }

        let escape_pressed = self.speech.as_ref().is_some_and(super::SpeechSystem::is_active)
            && ctx.input(|input| input.key_pressed(egui::Key::Escape));
        if escape_pressed {
            self.cancel_active_speech();
            self.speech_engaged_profile = None;
            self.speech_escape_cancelled = true;
        }

        self.poll_speech_runtime(ctx, events);
    }

    pub(in crate::app) fn sync_speech_hotkey_rebinding(&mut self, ctx: &Context) {
        let paused = super::super::shortcuts::hotkey_capture_active(ctx);
        if let Some(capture) = super::super::shortcuts::take_timed_out_hotkey_capture(ctx) {
            self.speech_global_hotkeys
                .quarantine_captured_key(capture.key, capture.physical_key);
        }
        self.speech_global_hotkeys.set_rebinding_paused(paused);
    }

    /// Drain global events and speech workers. Called from early-return views
    /// as well as the normal input path, so external dictation remains live
    /// while the board is empty or a startup view is visible.
    pub(in crate::app) fn poll_speech_runtime(&mut self, ctx: &Context, mut events: Vec<SpeechEvent>) {
        self.handle_global_speech_events(&mut events);
        self.revalidate_active_speech_target();

        let Some(speech) = self.speech.as_mut() else {
            ctx.data_mut(|data| data.remove_temp::<String>(egui::Id::new("speech_active_backend")));
            self.inject_speech_events(events);
            self.speech_external_targets.clear();
            return;
        };

        events.extend(speech.poll());
        let needs_poll = speech.is_active() || speech.has_pending_preloads();
        let active_backend = speech.active_backend().map(str::to_owned);
        let recording_finished = speech.recording_target().is_none();

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

        // Text events need the AX entry, so delivery must precede idle cleanup.
        self.inject_speech_events(events);
        if self
            .speech
            .as_ref()
            .and_then(super::SpeechSystem::active_target)
            .is_none()
        {
            self.speech_external_targets.clear();
        }
        if needs_poll {
            ctx.request_repaint_after(SPEECH_POLL_INTERVAL);
        }
    }

    fn handle_global_speech_events(&mut self, events: &mut Vec<SpeechEvent>) {
        let global_events = self.speech_global_hotkeys.drain_events();
        for event in global_events {
            let Some(activity) = self.speech.as_ref().map(speech_activity) else {
                continue;
            };
            let hotkey_mode = self
                .speech
                .as_ref()
                .map_or(horizon_core::SpeechHotkeyMode::Hold, super::SpeechSystem::hotkey_mode);
            match hotkey_mode {
                horizon_core::SpeechHotkeyMode::Hold => {
                    if event.pressed {
                        if activity != SpeechActivity::Idle || self.speech_engaged_profile.is_some() {
                            events.push(no_start_notice(activity));
                            continue;
                        }
                        let Some(target) = self.capture_global_speech_target(events) else {
                            continue;
                        };
                        let accepted = self
                            .speech
                            .as_mut()
                            .is_some_and(|speech| speech.start(target, event.profile));
                        if accepted {
                            self.speech_engaged_profile = Some(event.profile);
                        } else {
                            self.release_speech_target(target);
                            events.push(no_start_notice(
                                self.speech.as_ref().map_or(SpeechActivity::Idle, speech_activity),
                            ));
                        }
                    } else if self.speech_engaged_profile == Some(event.profile) {
                        if let Some(speech) = self.speech.as_mut() {
                            speech.stop();
                        }
                        self.speech_engaged_profile = None;
                    }
                }
                horizon_core::SpeechHotkeyMode::Toggle if event.pressed => {
                    if activity == SpeechActivity::Recording {
                        if let Some(speech) = self.speech.as_mut() {
                            speech.stop();
                        }
                    } else if activity == SpeechActivity::Busy {
                        events.push(no_start_notice(activity));
                    } else if let Some(target) = self.capture_global_speech_target(events) {
                        let accepted = self
                            .speech
                            .as_mut()
                            .is_some_and(|speech| speech.start(target, event.profile));
                        if !accepted {
                            self.release_speech_target(target);
                            events.push(no_start_notice(
                                self.speech.as_ref().map_or(SpeechActivity::Idle, speech_activity),
                            ));
                        }
                    }
                }
                horizon_core::SpeechHotkeyMode::Toggle => {}
            }
        }
    }

    #[cfg(all(test, feature = "speech"))]
    pub(in crate::app) fn handle_injected_global_speech_events(&mut self, events: &mut Vec<SpeechEvent>) {
        self.handle_global_speech_events(events);
    }

    fn capture_global_speech_target(&mut self, events: &mut Vec<SpeechEvent>) -> Option<SpeechTarget> {
        match self.speech_external_targets.capture() {
            Ok(FocusedTarget::Horizon) => {
                // Carbon also delivers while a detached Horizon viewport is
                // frontmost. Preserve the existing main-window-only terminal
                // hotkey policy instead of routing that press to whichever
                // main-board panel happened to remain focused.
                if !self.any_viewport_focused {
                    events.push(no_start_notice(SpeechActivity::Idle));
                    return None;
                }
                let search_capturing = self
                    .search_overlay
                    .as_ref()
                    .is_some_and(crate::search_overlay::SearchOverlay::input_focused);
                if self.text_surface_active(search_capturing) {
                    let surface = gated_press_surface(
                        self.settings.is_some(),
                        self.command_palette.is_some(),
                        search_capturing,
                    );
                    events.push(SpeechEvent::Notice(format!("Push-to-talk press ignored: {surface}.")));
                    None
                } else {
                    let target = self.focused_terminal_speech_target();
                    if target.is_none() {
                        events.push(no_start_notice(SpeechActivity::Idle));
                    }
                    target
                }
            }
            Ok(FocusedTarget::External(target)) => Some(SpeechTarget::External(target)),
            Err(error) => {
                if !self.speech_global_hotkeys.permission_is_current() {
                    self.speech_global_hotkeys.unregister_all();
                }
                tracing::info!(reason = %error, "external dictation target refused");
                events.push(SpeechEvent::Notice(error.to_string()));
                None
            }
        }
    }

    fn focused_terminal_speech_target(&self) -> Option<SpeechTarget> {
        self.board
            .focused
            .filter(|id| {
                self.board
                    .panel(*id)
                    .is_some_and(|panel| panel.terminal().is_some() && !self.workspace_is_detached(panel.workspace_id))
            })
            .map(SpeechTarget::Terminal)
    }

    fn text_surface_active(&self, search_capturing: bool) -> bool {
        self.settings.is_some()
            || self.command_palette.is_some()
            || search_capturing
            || self.renaming_panel.is_some()
            || self.renaming_workspace.is_some()
    }

    fn revalidate_active_speech_target(&mut self) {
        let Some(target) = self.speech.as_ref().and_then(super::SpeechSystem::active_target) else {
            return;
        };
        match target {
            SpeechTarget::Terminal(panel) if self.board.panel(panel).is_none() => {
                self.cancel_active_speech();
                self.speech_engaged_profile = None;
                tracing::info!("speech target panel disappeared; dictation cancelled");
            }
            SpeechTarget::External(target) => {
                if let Err(error) = self.speech_external_targets.revalidate_if_due(target) {
                    if !self.speech_global_hotkeys.permission_is_current() {
                        self.speech_global_hotkeys.unregister_all();
                    }
                    self.cancel_active_speech();
                    self.speech_engaged_profile = None;
                    tracing::info!(reason = %error, "external dictation target changed; result discarded");
                }
            }
            SpeechTarget::Terminal(_) => {}
        }
    }

    pub(in crate::app) fn cancel_active_speech(&mut self) {
        let target = self.speech.as_ref().and_then(super::SpeechSystem::active_target);
        if let Some(speech) = self.speech.as_mut() {
            speech.cancel();
        }
        if let Some(target) = target {
            self.release_speech_target(target);
        }
    }

    /// Cancel any active capture/inference and discard UI-thread target and
    /// queued-global ownership. Registrations stay installed so callers such
    /// as session/config transitions can keep the committed global binding set.
    pub(in crate::app) fn clear_speech_runtime_ownership(&mut self) {
        self.cancel_active_speech();
        self.speech_engaged_profile = None;
        self.speech_global_hotkeys.clear_event_ownership();
        self.speech_external_targets.clear();
        self.pending_terminal_speech.clear();
    }

    /// Release process-global keys before an application shutdown begins.
    /// This is deliberately idempotent because both the asynchronous close
    /// path and eframe's fallback exit callback can run it.
    pub(in crate::app) fn shutdown_speech_runtime(&mut self) {
        self.speech_global_hotkeys.unregister_all();
        self.clear_speech_runtime_ownership();
    }

    fn release_speech_target(&mut self, target: SpeechTarget) {
        if let SpeechTarget::External(target) = target {
            self.speech_external_targets.release(target);
        }
    }

    fn inject_speech_events(&mut self, events: Vec<SpeechEvent>) {
        for event in events {
            match event {
                SpeechEvent::Text { target, text } => match target {
                    SpeechTarget::Terminal(target) => {
                        if self.any_viewport_focused {
                            self.deliver_terminal_speech(target, &text);
                        } else {
                            self.pending_terminal_speech.push((target, text));
                        }
                    }
                    SpeechTarget::External(target) => {
                        let text = format!("{text} ");
                        if let Err(error) = self.speech_external_targets.insert_selected_text(target, &text) {
                            if !self.speech_global_hotkeys.permission_is_current() {
                                self.speech_global_hotkeys.unregister_all();
                            }
                            tracing::info!(reason = %error, "external dictation insertion refused");
                        }
                        self.speech_external_targets.release(target);
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

    fn deliver_terminal_speech(&mut self, target: horizon_core::PanelId, text: &str) {
        let Some(panel) = self.board.panel_mut(target) else {
            tracing::warn!("speech target panel closed before transcription finished");
            return;
        };
        let Some(mode) = panel.terminal().map(horizon_core::Terminal::mode) else {
            return;
        };
        let bytes = input::paste_bytes(&format!("{text} "), mode, true);
        panel.write_input(&bytes);
    }

    /// Resolve terminal results only after every detached viewport has added
    /// its focus to `any_viewport_focused`. External results bypass this
    /// queue because their exact AX target is independently revalidated.
    pub(in crate::app) fn finalize_pending_terminal_speech(&mut self) {
        let pending = std::mem::take(&mut self.pending_terminal_speech);
        if !self.any_viewport_focused {
            if !pending.is_empty() {
                tracing::info!(
                    count = pending.len(),
                    "terminal dictation result discarded after Horizon focus loss"
                );
            }
            return;
        }
        for (target, text) in pending {
            self.deliver_terminal_speech(target, &text);
        }
    }

    pub(in crate::app) fn show_speech_notice(&mut self, message: impl Into<String>, error: bool) {
        self.speech_notice = Some(crate::app::SpeechNotice {
            message: message.into(),
            error,
            shown_at: Instant::now(),
        });
    }

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

    /// Cancel terminal dictation only after every Horizon viewport rendered.
    /// External targets are expected to outlive Horizon focus and are guarded
    /// by exact AX revalidation instead.
    pub(in crate::app) fn cancel_unattended_recording(&mut self) {
        if self.any_viewport_focused {
            return;
        }
        if matches!(
            self.speech.as_ref().and_then(super::SpeechSystem::active_target),
            Some(SpeechTarget::Terminal(_))
        ) {
            self.speech_engaged_profile = None;
            self.cancel_active_speech();
            tracing::info!("all Horizon windows lost focus during terminal dictation; dictation cancelled");
        }
    }

    pub(in crate::app) fn stop_hold_on_focus_loss(&mut self, ctx: &Context, now: Instant) {
        self.arm_speech_release_ownership(ctx, now);
        let terminal_recording = matches!(
            self.speech.as_ref().and_then(super::SpeechSystem::recording_target),
            Some(SpeechTarget::Terminal(_))
        );
        if terminal_recording
            && self.speech_engaged_profile.is_some()
            && self
                .speech
                .as_ref()
                .is_some_and(|speech| speech.hotkey_mode() == horizon_core::SpeechHotkeyMode::Hold)
        {
            if let Some(speech) = self.speech.as_mut() {
                speech.stop();
            }
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

    pub(in crate::app) fn expire_speech_release_ownership(&mut self, now: Instant) {
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

    fn reset_speech_input_state(&mut self, root_focused: bool) {
        self.any_viewport_focused = root_focused;
        if !root_focused {
            self.speech_engaged_profile = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HoldHotkeyTransition, SpeechActivity, SpeechTarget, hold_hotkey_transition};
    use horizon_core::PanelId;

    #[test]
    fn hold_hotkey_claims_only_an_idle_session_with_a_target() {
        let focused = SpeechTarget::Terminal(PanelId(7));
        assert_eq!(
            hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, Some(focused)),
            HoldHotkeyTransition {
                start_target: Some(focused),
                stop: false,
                engaged_profile: Some(1),
            }
        );
        assert_eq!(
            hold_hotkey_transition(1, true, false, None, SpeechActivity::Idle, None),
            HoldHotkeyTransition {
                start_target: None,
                stop: false,
                engaged_profile: None,
            }
        );
    }

    #[test]
    fn hold_release_stops_only_the_engaged_profile() {
        let target = SpeechTarget::Terminal(PanelId(7));
        assert!(hold_hotkey_transition(1, false, true, Some(1), SpeechActivity::Recording, Some(target)).stop);
        assert!(!hold_hotkey_transition(2, false, true, Some(1), SpeechActivity::Recording, Some(target)).stop);
    }
}
