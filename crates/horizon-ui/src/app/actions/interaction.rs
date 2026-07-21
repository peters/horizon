use std::mem;

use egui::{Context, Event, Key, Modifiers, Rect, Vec2};
use horizon_core::WorkspaceId;

use super::super::super::input::{TerminalInputEvent, terminal_input_events};
use super::super::shortcuts::{event_uses_shortcut_key, shortcut_event_matches, shortcut_pressed};
use super::super::{CanvasPanSpaceKeyState, HorizonApp};
use super::support::fullscreen_panel_is_renderable;

impl CanvasPanSpaceKeyState {
    fn filter_terminal_events(
        &mut self,
        events: &[TerminalInputEvent],
        space_drag_claimed: bool,
    ) -> Vec<TerminalInputEvent> {
        let mut filtered = Vec::with_capacity(events.len());

        if space_drag_claimed && matches!(self, Self::Pending(_)) {
            *self = Self::Consumed;
        }

        for event in events {
            if self.handle_space_event(event, space_drag_claimed, &mut filtered) {
                continue;
            }

            if matches!(self, Self::Pending(_)) {
                filtered.extend(self.flush_pending());
            }

            filtered.push(event.clone());
        }

        filtered
    }

    fn handle_space_event(
        &mut self,
        event: &TerminalInputEvent,
        space_drag_claimed: bool,
        filtered: &mut Vec<TerminalInputEvent>,
    ) -> bool {
        match self {
            Self::Idle => {
                if is_space_pan_start_event(&event.event) {
                    if space_drag_claimed {
                        *self = Self::Consumed;
                    } else {
                        *self = Self::Pending(vec![event.clone()]);
                    }
                    return true;
                }
            }
            Self::Pending(pending) => {
                if is_space_pan_related_event(&event.event) {
                    pending.push(event.clone());
                    if space_drag_claimed {
                        *self = Self::Consumed;
                    } else if is_space_key_release(&event.event) {
                        filtered.extend(self.flush_pending());
                    }
                    return true;
                }
            }
            Self::Consumed => {
                if is_space_pan_related_event(&event.event) {
                    if is_space_key_release(&event.event) {
                        *self = Self::Idle;
                    }
                    return true;
                }
            }
        }

        false
    }

    fn flush_pending(&mut self) -> Vec<TerminalInputEvent> {
        match mem::take(self) {
            Self::Pending(events) => events,
            state => {
                *self = state;
                Vec::new()
            }
        }
    }
}

fn is_space_pan_start_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Key {
            key: Key::Space,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } if space_drag_modifier_active(*modifiers)
    )
}

fn is_space_pan_related_event(event: &Event) -> bool {
    matches!(event, Event::Key { key: Key::Space, .. })
        || matches!(event, Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) if text == " ")
}

fn is_space_key_release(event: &Event) -> bool {
    matches!(
        event,
        Event::Key {
            key: Key::Space,
            pressed: false,
            ..
        }
    )
}

fn space_drag_modifier_active(modifiers: Modifiers) -> bool {
    !modifiers.ctrl && !modifiers.command && !modifiers.alt
}

// egui feeds every wheel/trackpad event into both raw_scroll_delta and
// smooth_scroll_delta; summing them would pan the canvas twice per event.
fn wheel_pan_scroll_input(input: &egui::InputState) -> Vec2 {
    input.smooth_scroll_delta
}

impl HorizonApp {
    pub(in super::super) fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
        // A chord being captured by the settings hotkey binder must not
        // trigger the shortcut it happens to match.
        if super::super::shortcuts::hotkey_capture_active(ctx) {
            return;
        }
        let (panel_toggle, window_toggle, exit_fullscreen) = ctx.input(|input| {
            (
                shortcut_pressed(input, self.shortcuts.fullscreen_panel),
                shortcut_pressed(input, self.shortcuts.fullscreen_window),
                shortcut_pressed(input, self.shortcuts.exit_fullscreen_panel),
            )
        });

        if window_toggle {
            let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if panel_toggle {
            self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                None
            } else {
                self.board.focused
            };
        } else if exit_fullscreen && self.fullscreen_panel.is_some() && !self.speech_escape_cancelled {
            self.fullscreen_panel = None;
        }

        if let Some(panel_id) = self.fullscreen_panel
            && !fullscreen_panel_is_renderable(&self.board, &self.detached_workspaces, panel_id)
        {
            self.fullscreen_panel = None;
        }
    }

    #[profiling::function]
    pub(in super::super) fn handle_canvas_pan(&mut self, ctx: &Context) {
        self.handle_canvas_pan_in_rect(ctx, self.canvas_rect(ctx), None);
    }

    #[profiling::function]
    pub(in super::super) fn handle_canvas_pan_in_rect(
        &mut self,
        ctx: &Context,
        canvas_rect: Rect,
        visible_workspace: Option<WorkspaceId>,
    ) {
        let (
            events,
            pointer_position,
            middle_down,
            primary_down,
            space_down,
            modifiers,
            scroll,
            pointer_delta,
            zoom_delta,
        ) = ctx.input(|input| {
            (
                input.events.clone(),
                input.pointer.interact_pos().or_else(|| input.pointer.hover_pos()),
                input.pointer.middle_down(),
                input.pointer.primary_down(),
                input.key_down(egui::Key::Space),
                input.modifiers,
                wheel_pan_scroll_input(input),
                input.pointer.delta(),
                input.zoom_delta(),
            )
        });
        let panel_geometry = self.visible_panel_geometry_for_canvas_view(canvas_rect, visible_workspace);
        let pointer_in_canvas = pointer_position.is_some_and(|position| canvas_rect.contains(position));
        let space_drag_claimed =
            pointer_in_canvas && primary_down && space_down && space_drag_modifier_active(modifiers);
        let ctrl_or_cmd = modifiers.ctrl || modifiers.command;
        let pointer_over_terminal_body = primary_selection_routing_active()
            && pointer_position.is_some_and(|position| {
                panel_geometry
                    .iter()
                    .filter_map(|(_, geometry)| geometry.terminal_body_screen_rect)
                    .any(|rect| rect.contains(position))
            });
        let terminal_events = self.terminal_events_for_viewport(ctx, &events);
        // Delay plain Space forwarding until we know whether the key becomes
        // the canvas-pan modifier or an actual terminal keystroke.
        self.terminal_keyboard_events = self
            .pending_space_pan_key
            .filter_terminal_events(&terminal_events, space_drag_claimed);
        let target = if !pointer_in_canvas {
            MiddlePanTarget::OutsideCanvas
        } else if pointer_over_terminal_body {
            MiddlePanTarget::TerminalBody
        } else {
            MiddlePanTarget::EmptyCanvas
        };
        let mode = if ctrl_or_cmd {
            MiddlePanMode::Forced
        } else {
            MiddlePanMode::Default
        };
        self.middle_pan_active =
            next_middle_pan_active(self.middle_pan_active, middle_down, target, mode, pointer_delta);
        self.canvas_pan_input_claimed = pointer_in_canvas && (self.middle_pan_active || space_drag_claimed);
        if pointer_in_canvas && (zoom_delta - 1.0).abs() > f32::EPSILON {
            let anchor = pointer_position.unwrap_or_else(|| canvas_rect.center());
            if self.zoom_canvas_at(canvas_rect, anchor, self.canvas_view.zoom * zoom_delta) {
                self.clear_terminal_selections();
            }
            self.canvas_pan_input_claimed = false;
            self.is_panning = false;
            return;
        }

        let drag_panning = self.canvas_pan_input_claimed;
        let pointer_over_panel = pointer_position.is_some_and(|position| {
            pointer_in_canvas
                && !drag_panning
                && scroll != Vec2::ZERO
                && !ctrl_or_cmd
                && panel_geometry
                    .iter()
                    .any(|(_, geometry)| geometry.screen_rect.contains(position))
        });
        let pan_delta = if drag_panning {
            pointer_delta
        } else if pointer_in_canvas && !pointer_over_panel && !ctrl_or_cmd {
            if modifiers.shift && scroll.x == 0.0 {
                Vec2::new(scroll.y, 0.0)
            } else {
                scroll
            }
        } else {
            Vec2::ZERO
        };

        self.is_panning = pan_delta != Vec2::ZERO;
        if self.is_panning {
            self.pan_target = None;
            let mut pan_offset = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
            pan_offset += pan_delta;
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
            self.mark_runtime_dirty();
            self.clear_terminal_selections();
        }
    }

    fn clear_terminal_selections(&self) {
        for panel in &self.board.panels {
            if let Some(terminal) = panel.terminal() {
                terminal.clear_selection();
            }
        }
    }

    fn terminal_events_for_viewport(&mut self, ctx: &Context, events: &[Event]) -> Vec<TerminalInputEvent> {
        let viewport_id = ctx.viewport_id();
        let frame_keyboard_events = self.frame_keyboard_events.remove(&viewport_id).unwrap_or_default();
        // The push-to-talk chord is an app-level control on the root viewport
        // (where the hotkey handler listens); keep its presses, repeats, and
        // release out of the PTY stream without swallowing unrelated keys.
        // A detached viewport does not run that filter, but Escape must still
        // cancel a recording started from its own mic button (dictation is
        // cancellable everywhere), so handle just that here.
        if viewport_id != egui::ViewportId::ROOT {
            if self.speech.as_ref().is_some_and(|s| s.recording_target().is_some()) {
                let escape_pressed = events.iter().any(|event| {
                    matches!(
                        event,
                        Event::Key {
                            key: Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                });
                if escape_pressed {
                    if let Some(speech) = self.speech.as_mut() {
                        speech.cancel();
                    }
                    self.speech_engaged_profile = None;
                    let filtered: Vec<Event> = events
                        .iter()
                        .filter(|event| !matches!(event, Event::Key { key: Key::Escape, .. }))
                        .cloned()
                        .collect();
                    return terminal_input_events(&filtered, frame_keyboard_events);
                }
            }
            return terminal_input_events(events, frame_keyboard_events);
        }
        // While the settings binder is actively capturing, every key
        // belongs to the binder. Use the NARROW flag (not the combined
        // capture-active that also covers the release-pending grace period),
        // so the pending-key branch below still runs to consume and clear
        // the captured chord's release instead of this branch eating it.
        let capturing = super::super::shortcuts::hotkey_binder_capturing(ctx);
        // A just-captured chord still has its release (and possibly repeats
        // and a text event) in flight after the binder cleared its flag.
        let captured_key_id = egui::Id::new("speech_captured_key");
        let captured: Option<(Key, bool)> = ctx
            .data(|data| data.get_temp::<Option<(Key, bool)>>(captured_key_id))
            .flatten();
        let mut filtered = Vec::with_capacity(events.len());
        for event in events {
            if capturing
                && matches!(
                    event,
                    Event::Key { .. } | Event::Text(_) | Event::Copy | Event::Cut | Event::Paste(_) | Event::Ime(_)
                )
            {
                // Every input belongs to the binder while it captures — incl.
                // the synthetic Copy/Cut/Paste egui-winit emits for Ctrl+C/X/V
                // and IME commits — so none reaches the focused terminal.
                continue;
            }
            if let Some((pending, shifted)) = captured {
                match event {
                    Event::Key {
                        key, pressed: false, ..
                    } if keys_equivalent(*key, pending) => {
                        ctx.data_mut(|data| data.insert_temp(captured_key_id, None::<(Key, bool)>));
                        continue;
                    }
                    Event::Key { key, .. } if keys_equivalent(*key, pending) => continue,
                    // A shifted digit/punctuation key emits a layout-dependent
                    // symbol (Shift+1 -> "!"); while that chord is the only key
                    // held, swallow its single-character text too.
                    Event::Text(text) if key_emits_text(pending, text) => continue,
                    Event::Text(text) if shifted && text.chars().count() == 1 => continue,
                    _ => {}
                }
            }
            let bindings = self
                .speech
                .as_ref()
                .map_or(&[][..], super::super::speech::SpeechSystem::profile_bindings);
            if swallow_speech_hotkey_event(
                &mut self.speech_held_bindings,
                self.speech_escape_cancelled,
                &mut self.speech_escape_release_pending,
                event,
                bindings,
            ) {
                continue;
            }
            filtered.push(event.clone());
        }
        terminal_input_events(&filtered, frame_keyboard_events)
    }
}

/// Whether a root-viewport event belongs to a push-to-talk chord (or to
/// an Escape that cancelled dictation) and must not reach the terminal.
///
/// Presses are swallowed only on a full chord match, so a modifier hotkey
/// like `Ctrl+K` does not eat bare `K` keystrokes. Every held chord is
/// tracked independently until its release is seen — modifiers are often
/// released before the main key, and multiple profile keys can be down
/// at once. The Escape consumption applies even with no hotkey configured,
/// because mic-button dictation is cancelled with Escape too.
///
/// Free function over disjoint borrows so the caller can keep the binding
/// slice borrowed from the speech system (hot path: the binding list must
/// not be cloned per frame).
fn swallow_speech_hotkey_event(
    held_bindings: &mut Vec<horizon_core::ShortcutBinding>,
    escape_cancelled: bool,
    escape_release_pending: &mut bool,
    event: &Event,
    bindings: &[(usize, horizon_core::ShortcutBinding)],
) -> bool {
    if let Event::Key {
        key: Key::Escape,
        pressed,
        ..
    } = event
    {
        // A cancel-Escape is swallowed as a press AND as its later release:
        // kitty REPORT_EVENT_TYPES terminals would otherwise receive a
        // dangling Escape-release CSI-u sequence next frame.
        if escape_cancelled && *pressed {
            *escape_release_pending = true;
            return true;
        }
        if *escape_release_pending && !pressed {
            *escape_release_pending = false;
            return true;
        }
    }
    match event {
        Event::Key { pressed, .. } => {
            if *pressed {
                // A full chord press of any profile key engages holding.
                if let Some((_, matched)) = bindings
                    .iter()
                    .find(|(_, binding)| shortcut_event_matches(event, *binding))
                {
                    if !held_bindings.contains(matched) {
                        held_bindings.push(*matched);
                    }
                    return true;
                }
                // Mid-hold repeats can carry drifted modifiers.
                held_bindings.iter().any(|held| event_uses_shortcut_key(event, *held))
            } else if held_bindings.iter().any(|held| event_uses_shortcut_key(event, *held)) {
                // A physical-key release clears EVERY held chord on that key
                // (distinct chords like Ctrl+K and Alt+K can both be held via
                // modifier drift), so no entry is left stuck to swallow that
                // key forever. Chords on other keys stay held.
                held_bindings.retain(|held| !event_uses_shortcut_key(event, *held));
                true
            } else {
                false
            }
        }
        Event::Text(text) => {
            // A held chord's own key emits Text while held — directly (no
            // modifier), under Shift, under Alt (bare letter on Linux), and
            // during modifier-drift repeats (Ctrl+K with Ctrl released emits
            // Text("k")). Swallow only Text matching a HELD key's glyph, so
            // unrelated keys typed while holding push-to-talk still reach the
            // terminal (e.g. "L" while holding Shift+K).
            held_bindings.iter().any(|held| binding_text_matches(held.key, text))
        }
        _ => false,
    }
}

/// egui reports the `+`/`=` keycap as `Plus` on press but `Equals` on
/// release when Shift is dropped first; treat them as one key so a pending
/// capture clears on release.
fn keys_equivalent(a: Key, b: Key) -> bool {
    a == b || matches!((a, b), (Key::Plus, Key::Equals) | (Key::Equals, Key::Plus))
}

/// The text a captured key emits alongside its key event (`Key::name()`
/// returns display names like `Comma`, not the `,` glyph terminals see).
fn key_emits_text(key: Key, text: &str) -> bool {
    match key {
        Key::Comma => text == ",",
        Key::Minus => text == "-",
        Key::Plus => text == "+",
        Key::Equals => text == "=",
        Key::Period => text == ".",
        _ => text.eq_ignore_ascii_case(key.name()),
    }
}

/// The text a modifier-free shortcut key emits alongside its key event.
fn binding_text_matches(key: horizon_core::ShortcutKey, text: &str) -> bool {
    match key {
        horizon_core::ShortcutKey::Letter(letter) => text.eq_ignore_ascii_case(&letter.to_string()),
        horizon_core::ShortcutKey::Digit(digit) => text == digit.to_string(),
        horizon_core::ShortcutKey::Comma => text == ",",
        horizon_core::ShortcutKey::Minus => text == "-",
        // `Plus` binds both the + and = keycaps.
        horizon_core::ShortcutKey::Plus => text == "+" || text == "=",
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum MiddlePanTarget {
    OutsideCanvas,
    EmptyCanvas,
    TerminalBody,
}

#[derive(Clone, Copy)]
enum MiddlePanMode {
    Default,
    Forced,
}

fn next_middle_pan_active(
    was_active: bool,
    middle_down: bool,
    target: MiddlePanTarget,
    mode: MiddlePanMode,
    pointer_delta: Vec2,
) -> bool {
    if !middle_down {
        return false;
    }

    if was_active {
        return true;
    }

    if pointer_delta == Vec2::ZERO {
        return false;
    }

    match (target, mode) {
        (MiddlePanTarget::OutsideCanvas, _) | (MiddlePanTarget::TerminalBody, MiddlePanMode::Default) => false,
        (MiddlePanTarget::EmptyCanvas, _) | (MiddlePanTarget::TerminalBody, MiddlePanMode::Forced) => true,
    }
}

fn primary_selection_routing_active() -> bool {
    cfg!(target_os = "linux")
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, Vec2};

    use super::super::super::super::input::TerminalInputEvent;
    use super::super::super::CanvasPanSpaceKeyState;
    use super::{
        MiddlePanMode, MiddlePanTarget, next_middle_pan_active, primary_selection_routing_active,
        wheel_pan_scroll_input,
    };

    #[test]
    fn wheel_pan_scroll_input_counts_each_wheel_event_once() {
        let delta = Vec2::new(3.0, -5.0);
        let raw_input = egui::RawInput {
            events: vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                modifiers: Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };

        let input = egui::InputState::default().begin_pass(raw_input, false, 1.0, egui::InputOptions::default());

        // A point-unit delta below egui's smoothing threshold lands in full in
        // both raw_scroll_delta and smooth_scroll_delta within the same pass,
        // so reading both would double every trackpad gesture.
        assert_eq!(input.raw_scroll_delta, delta);
        assert_eq!(input.smooth_scroll_delta, delta);
        assert_eq!(wheel_pan_scroll_input(&input), delta);
    }

    #[test]
    fn wheel_pan_scroll_input_reads_only_the_smoothed_delta_for_notched_wheels() {
        let raw_input = egui::RawInput {
            events: vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -14.0),
                modifiers: Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };

        let input = egui::InputState::default().begin_pass(raw_input, false, 1.0 / 60.0, egui::InputOptions::default());

        // Line-unit notches bypass egui's smoothing threshold, so the raw and
        // smoothed deltas diverge within one pass. That divergence is what makes
        // this assertion discriminating: the point-unit case above passes for
        // either field, so without this a regression to the raw delta — the
        // exact doubling this fix removes — would go undetected.
        assert_ne!(input.raw_scroll_delta, input.smooth_scroll_delta);
        assert_eq!(wheel_pan_scroll_input(&input), input.smooth_scroll_delta);
    }

    #[test]
    fn plain_space_is_delayed_until_release() {
        let mut state = CanvasPanSpaceKeyState::default();
        let press = space_press();
        let text = Event::Text(" ".to_owned());
        let release = space_release();

        assert!(
            state
                .filter_terminal_events(&[terminal_event(press.clone()), terminal_event(text.clone())], false)
                .is_empty()
        );

        let filtered = state.filter_terminal_events(&[terminal_event(release.clone())], false);
        assert_eq!(
            filtered,
            vec![terminal_event(press), terminal_event(text), terminal_event(release)]
        );
        assert!(matches!(state, CanvasPanSpaceKeyState::Idle));
    }

    #[test]
    fn space_candidate_is_dropped_once_drag_pan_claims_it() {
        let mut state = CanvasPanSpaceKeyState::default();

        assert!(
            state
                .filter_terminal_events(
                    &[
                        terminal_event(space_press()),
                        terminal_event(Event::Text(" ".to_owned()))
                    ],
                    false
                )
                .is_empty()
        );
        assert!(matches!(state, CanvasPanSpaceKeyState::Pending(_)));

        assert!(state.filter_terminal_events(&[], true).is_empty());
        assert!(matches!(state, CanvasPanSpaceKeyState::Consumed));

        assert!(
            state
                .filter_terminal_events(&[terminal_event(space_release())], false)
                .is_empty()
        );
        assert!(matches!(state, CanvasPanSpaceKeyState::Idle));
    }

    #[test]
    fn pending_space_flushes_before_later_non_space_input() {
        let mut state = CanvasPanSpaceKeyState::default();
        let press = space_press();
        let text = Event::Text(" ".to_owned());
        let letter = Event::Key {
            key: Key::A,
            physical_key: Some(Key::A),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };

        assert!(
            state
                .filter_terminal_events(&[terminal_event(press.clone()), terminal_event(text.clone())], false)
                .is_empty()
        );

        let filtered = state.filter_terminal_events(&[terminal_event(letter.clone())], false);
        assert_eq!(
            filtered,
            vec![terminal_event(press), terminal_event(text), terminal_event(letter)]
        );
        assert!(matches!(state, CanvasPanSpaceKeyState::Idle));
    }

    #[test]
    fn middle_pan_starts_on_empty_canvas() {
        assert!(next_middle_pan_active(
            false,
            true,
            MiddlePanTarget::EmptyCanvas,
            MiddlePanMode::Default,
            Vec2::new(4.0, 0.0)
        ));
    }

    #[test]
    fn middle_pan_does_not_start_on_terminal_body_without_modifier() {
        assert!(!next_middle_pan_active(
            false,
            true,
            MiddlePanTarget::TerminalBody,
            MiddlePanMode::Default,
            Vec2::new(4.0, 0.0)
        ));
    }

    #[test]
    fn middle_pan_overrides_terminal_body_with_ctrl_or_cmd() {
        assert!(next_middle_pan_active(
            false,
            true,
            MiddlePanTarget::TerminalBody,
            MiddlePanMode::Forced,
            Vec2::new(4.0, 0.0)
        ));
    }

    #[test]
    fn middle_pan_stays_active_until_button_release() {
        assert!(next_middle_pan_active(
            true,
            true,
            MiddlePanTarget::OutsideCanvas,
            MiddlePanMode::Default,
            Vec2::ZERO
        ));
        assert!(!next_middle_pan_active(
            true,
            false,
            MiddlePanTarget::EmptyCanvas,
            MiddlePanMode::Default,
            Vec2::ZERO
        ));
    }

    #[test]
    fn middle_pan_waits_for_motion_before_claiming_press() {
        assert!(!next_middle_pan_active(
            false,
            true,
            MiddlePanTarget::EmptyCanvas,
            MiddlePanMode::Default,
            Vec2::ZERO
        ));
    }

    #[test]
    fn primary_selection_routing_matches_linux_only_behavior() {
        assert_eq!(primary_selection_routing_active(), cfg!(target_os = "linux"));
    }

    fn terminal_event(event: Event) -> TerminalInputEvent {
        TerminalInputEvent {
            event,
            key_without_modifiers_text: None,
            observed_key: None,
        }
    }

    fn space_press() -> Event {
        Event::Key {
            key: Key::Space,
            physical_key: Some(Key::Space),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn space_release() -> Event {
        Event::Key {
            key: Key::Space,
            physical_key: Some(Key::Space),
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }
}
