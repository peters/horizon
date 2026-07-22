use std::mem;

use egui::{Context, Event, Key, Modifiers, Rect, Vec2};
use horizon_core::WorkspaceId;

use super::super::super::input::{TerminalInputEvent, terminal_input_events};
use super::super::shortcuts::{
    event_uses_shortcut_key, is_clipboard_pseudo_event, pending_hotkey_capture, shortcut_event_matches,
    shortcut_pressed, take_captured_clipboard_event,
};
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
            if let Some(filtered) = self.filter_detached_cancel_escape(events) {
                return terminal_input_events(&filtered, frame_keyboard_events);
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
        // This accessor applies the same timeout used by global shortcut
        // dispatch, so neither path can be wedged by a lost release.
        let captured = pending_hotkey_capture(ctx);
        let captured_clipboard_event = take_captured_clipboard_event(ctx);
        let mut filtered = Vec::with_capacity(events.len());
        let mut swallow_next_shift_text = false;
        for event in events {
            if swallow_correlated_shift_text(&mut swallow_next_shift_text, event) {
                continue;
            }
            if swallow_captured_clipboard_event(captured_clipboard_event, event) {
                continue;
            }
            if capturing {
                // Rebind owns the input, but a release from a hold that began
                // before capture still has to retire the terminal filter's
                // held entry before the event is consumed below.
                clear_released_speech_hotkeys(&mut self.speech_held_bindings, event);
            }
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
            if let Some(pending) = captured {
                match pending_capture_event(pending, event) {
                    PendingCaptureEvent::Release => {
                        ctx.data_mut(|data| {
                            data.insert_temp(captured_key_id, None::<super::super::settings::PendingCapture>);
                        });
                        continue;
                    }
                    PendingCaptureEvent::SwallowAndArmShiftText => {
                        swallow_next_shift_text = true;
                        continue;
                    }
                    PendingCaptureEvent::Swallow => continue,
                    PendingCaptureEvent::Forward => {}
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
                &mut swallow_next_shift_text,
                event,
                bindings,
            ) {
                continue;
            }
            filtered.push(event.clone());
        }
        terminal_input_events(&filtered, frame_keyboard_events)
    }

    fn filter_detached_cancel_escape(&mut self, events: &[Event]) -> Option<Vec<Event>> {
        let escape_cancelled = self.speech.as_ref().is_some_and(|speech| {
            speech.recording_target().is_some()
                && events.iter().any(|event| {
                    matches!(
                        event,
                        Event::Key {
                            key: Key::Escape,
                            pressed: true,
                            ..
                        }
                    )
                })
        });
        if escape_cancelled {
            if let Some(speech) = self.speech.as_mut() {
                speech.cancel();
            }
            self.speech_engaged_profile = None;
        }
        if !escape_cancelled && !self.speech_escape_release_pending {
            return None;
        }

        let filtered = events
            .iter()
            .filter(|event| {
                !swallow_cancel_escape_event(escape_cancelled, &mut self.speech_escape_release_pending, event)
            })
            .cloned()
            .collect();
        Some(filtered)
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
    swallow_next_shift_text: &mut bool,
    event: &Event,
    bindings: &[(usize, horizon_core::ShortcutBinding)],
) -> bool {
    if swallow_cancel_escape_event(escape_cancelled, escape_release_pending, event) {
        return true;
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
                    arm_correlated_shift_text(*matched, swallow_next_shift_text);
                    return true;
                }
                // Mid-hold repeats can carry drifted modifiers.
                if let Some(held) = held_bindings.iter().find(|held| event_uses_shortcut_key(event, **held)) {
                    arm_correlated_shift_text(*held, swallow_next_shift_text);
                    true
                } else {
                    false
                }
            } else {
                clear_released_speech_hotkeys(held_bindings, event)
            }
        }
        // Direct glyphs and modifier-drift repeats can emit Text without an
        // adjacent shifted key event. Layout-dependent shifted glyphs are
        // handled by the event-correlation flag armed above.
        Event::Text(text) => held_bindings.iter().any(|held| binding_text_matches(held.key, text)),
        _ => false,
    }
}

/// A physical-key release clears every held chord on that key. Distinct
/// chords such as Ctrl+K and Alt+K can both be present after modifier drift;
/// clearing all matches prevents a permanent terminal-input swallow.
fn clear_released_speech_hotkeys(held_bindings: &mut Vec<horizon_core::ShortcutBinding>, event: &Event) -> bool {
    let Event::Key { pressed: false, .. } = event else {
        return false;
    };
    let matched = held_bindings.iter().any(|held| event_uses_shortcut_key(event, *held));
    if matched {
        held_bindings.retain(|held| !event_uses_shortcut_key(event, *held));
    }
    matched
}

/// Consume a cancel-Escape's press, repeats, and later release. The pending
/// state is shared across viewport frames so a detached terminal cannot
/// receive a dangling kitty release event after dictation was cancelled.
fn swallow_cancel_escape_event(escape_cancelled: bool, escape_release_pending: &mut bool, event: &Event) -> bool {
    let Event::Key {
        key: Key::Escape,
        pressed,
        ..
    } = event
    else {
        return false;
    };
    if escape_cancelled && *pressed {
        *escape_release_pending = true;
        return true;
    }
    if *escape_release_pending {
        if !pressed {
            *escape_release_pending = false;
        }
        return true;
    }
    false
}

/// A consumed shifted printable key is followed immediately by its Text
/// event in egui-winit's event batch. Consume only that adjacent Text; taking
/// the flag here ensures any intervening event cancels the correlation.
fn swallow_correlated_shift_text(pending: &mut bool, event: &Event) -> bool {
    std::mem::take(pending) && matches!(event, Event::Text(_))
}

fn swallow_captured_clipboard_event(captured: bool, event: &Event) -> bool {
    captured && is_clipboard_pseudo_event(event)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingCaptureEvent {
    Forward,
    Swallow,
    SwallowAndArmShiftText,
    Release,
}

fn pending_capture_event(pending: super::super::settings::PendingCapture, event: &Event) -> PendingCaptureEvent {
    // Match the PHYSICAL key as well: a shifted keycap presses as one logical
    // key (Shift+1 -> Exclamationmark) and releases as another (Num1) once
    // Shift is lifted first.
    let matches_pending = |key: Key, physical: Option<Key>| {
        keys_equivalent(key, pending.key)
            || (pending.physical_key.is_some() && physical == pending.physical_key)
            || pending
                .clipboard
                .is_some_and(|clipboard| clipboard.matches_release_key(key))
    };
    match event {
        Event::Key {
            key,
            physical_key,
            pressed: false,
            ..
        } if matches_pending(*key, *physical_key) => PendingCaptureEvent::Release,
        Event::Key {
            key,
            physical_key,
            pressed: true,
            ..
        } if matches_pending(*key, *physical_key) && pending.shifted && egui_key_may_emit_text(*key) => {
            PendingCaptureEvent::SwallowAndArmShiftText
        }
        Event::Key { key, physical_key, .. } if matches_pending(*key, *physical_key) => PendingCaptureEvent::Swallow,
        Event::Text(text) if key_emits_text(pending.key, text) => PendingCaptureEvent::Swallow,
        _ => PendingCaptureEvent::Forward,
    }
}

fn arm_correlated_shift_text(binding: horizon_core::ShortcutBinding, pending: &mut bool) {
    if binding.modifiers.shift() && shortcut_key_may_emit_text(binding.key) {
        *pending = true;
    }
}

fn shortcut_key_may_emit_text(key: horizon_core::ShortcutKey) -> bool {
    matches!(
        key,
        horizon_core::ShortcutKey::Letter(_)
            | horizon_core::ShortcutKey::Digit(_)
            | horizon_core::ShortcutKey::Comma
            | horizon_core::ShortcutKey::Minus
            | horizon_core::ShortcutKey::Plus
    )
}

/// egui reports the `+`/`=` keycap as `Plus` on press but `Equals` on
/// release when Shift is dropped first; treat them as one key so a pending
/// capture clears on release.
fn keys_equivalent(a: Key, b: Key) -> bool {
    a == b || matches!((a, b), (Key::Plus, Key::Equals) | (Key::Equals, Key::Plus))
}

fn egui_key_may_emit_text(key: Key) -> bool {
    key == Key::Space
        || key == Key::Quote
        || (key.symbol_or_name().chars().count() == 1
            && !matches!(key, Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp))
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
    use horizon_core::{ShortcutBinding, ShortcutKey, ShortcutModifiers};

    use super::super::super::super::input::TerminalInputEvent;
    use super::super::super::CanvasPanSpaceKeyState;
    use super::{
        MiddlePanMode, MiddlePanTarget, PendingCaptureEvent, clear_released_speech_hotkeys, next_middle_pan_active,
        pending_capture_event, primary_selection_routing_active, swallow_cancel_escape_event,
        swallow_captured_clipboard_event, swallow_correlated_shift_text, swallow_speech_hotkey_event,
        wheel_pan_scroll_input,
    };

    #[test]
    fn captured_clipboard_marker_swallows_pseudo_events_only() {
        for event in [Event::Copy, Event::Cut, Event::Paste("text".to_string())] {
            assert!(swallow_captured_clipboard_event(true, &event));
            assert!(!swallow_captured_clipboard_event(false, &event));
        }
        assert!(!swallow_captured_clipboard_event(
            true,
            &Event::Key {
                key: Key::C,
                physical_key: Some(Key::C),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }
        ));
    }

    #[test]
    fn clipboard_key_releases_in_a_later_frame_are_claimed() {
        use crate::app::settings::ClipboardCapture;

        // Frame one contains only the pseudo-event. egui-winit can produce it
        // from the primary C/X/V chord, Windows Insert/Delete conventions, or
        // a dedicated clipboard key, then delivers the ordinary release later.
        for (clipboard, fallback, release_keys) in [
            (ClipboardCapture::Copy, Key::C, [Key::C, Key::Insert, Key::Copy]),
            (ClipboardCapture::Cut, Key::X, [Key::X, Key::Delete, Key::Cut]),
            (ClipboardCapture::Paste, Key::V, [Key::V, Key::Insert, Key::Paste]),
        ] {
            let pending = crate::app::settings::PendingCapture {
                key: fallback,
                physical_key: Some(fallback),
                shifted: false,
                clipboard: Some(clipboard),
                armed_at: 1.0,
            };
            for key in release_keys {
                let release = Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: false,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                };
                assert_eq!(pending_capture_event(pending, &release), PendingCaptureEvent::Release);
            }
        }
    }

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

    #[test]
    fn cancel_escape_swallows_release_in_a_later_batch() {
        let mut release_pending = false;
        assert!(swallow_cancel_escape_event(
            true,
            &mut release_pending,
            &key_event(Key::Escape, Some(Key::Escape), true, Modifiers::NONE)
        ));
        assert!(release_pending);

        assert!(swallow_cancel_escape_event(
            false,
            &mut release_pending,
            &key_event(Key::Escape, Some(Key::Escape), false, Modifiers::NONE)
        ));
        assert!(!release_pending);
    }

    #[test]
    fn cancel_escape_press_and_release_clear_pending_in_one_batch() {
        let mut release_pending = false;
        for event in [
            key_event(Key::Escape, Some(Key::Escape), true, Modifiers::NONE),
            key_event(Key::Escape, Some(Key::Escape), false, Modifiers::NONE),
        ] {
            assert!(swallow_cancel_escape_event(true, &mut release_pending, &event));
        }
        assert!(!release_pending);
    }

    #[test]
    fn shifted_hotkey_text_is_correlated_without_swallowing_other_keys() {
        let binding = ShortcutBinding::new(ShortcutModifiers::SHIFT, ShortcutKey::Letter('K'));
        let bindings = [(0, binding)];
        let mut held = Vec::new();
        let mut escape_release_pending = false;
        let mut shift_text_pending = false;

        assert!(speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &key_event(Key::K, Some(Key::K), true, Modifiers::SHIFT),
            &bindings,
        ));
        assert!(speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &Event::Text("K".to_string()),
            &bindings,
        ));
        assert!(!speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &key_event(Key::L, Some(Key::L), true, Modifiers::SHIFT),
            &bindings,
        ));
        assert!(!speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &Event::Text("L".to_string()),
            &bindings,
        ));
    }

    #[test]
    fn shifted_digit_hotkey_swallows_its_layout_glyph() {
        let binding = ShortcutBinding::new(ShortcutModifiers::SHIFT, ShortcutKey::Digit(1));
        let bindings = [(0, binding)];
        let mut held = Vec::new();
        let mut escape_release_pending = false;
        let mut shift_text_pending = false;

        assert!(speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &key_event(Key::Exclamationmark, Some(Key::Num1), true, Modifiers::SHIFT,),
            &bindings,
        ));
        assert!(speech_event_swallowed(
            &mut held,
            &mut escape_release_pending,
            &mut shift_text_pending,
            &Event::Text("!".to_string()),
            &bindings,
        ));
    }

    #[test]
    fn rebind_capture_release_clears_held_speech_filter() {
        let binding = ShortcutBinding::new(ShortcutModifiers::CTRL, ShortcutKey::Letter('K'));
        let other = ShortcutBinding::new(ShortcutModifiers::ALT, ShortcutKey::Letter('L'));
        let mut held = vec![binding, other];
        let release = key_event(Key::K, Some(Key::K), false, Modifiers::NONE);

        assert!(clear_released_speech_hotkeys(&mut held, &release));
        assert_eq!(held, vec![other]);
    }

    fn speech_event_swallowed(
        held: &mut Vec<ShortcutBinding>,
        escape_release_pending: &mut bool,
        shift_text_pending: &mut bool,
        event: &Event,
        bindings: &[(usize, ShortcutBinding)],
    ) -> bool {
        swallow_correlated_shift_text(shift_text_pending, event)
            || swallow_speech_hotkey_event(held, false, escape_release_pending, shift_text_pending, event, bindings)
    }

    fn terminal_event(event: Event) -> TerminalInputEvent {
        TerminalInputEvent {
            event,
            key_without_modifiers_text: None,
            observed_key: None,
        }
    }

    fn key_event(key: Key, physical_key: Option<Key>, pressed: bool, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key,
            pressed,
            repeat: false,
            modifiers,
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
