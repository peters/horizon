use std::mem;

use egui::{Context, Event, Key, Modifiers, Rect, Vec2};

use super::super::super::input::{TerminalInputEvent, terminal_input_events};
use super::super::shortcuts::shortcut_pressed;
use super::super::{CanvasPanSpaceKeyState, HorizonApp};

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

impl HorizonApp {
    pub(in super::super) fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
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
        } else if exit_fullscreen && self.fullscreen_panel.is_some() {
            self.fullscreen_panel = None;
        }

        if let Some(panel_id) = self.fullscreen_panel
            && self.board.panel(panel_id).is_none()
        {
            self.fullscreen_panel = None;
        }
    }

    #[profiling::function]
    pub(in super::super) fn handle_canvas_pan(&mut self, ctx: &Context) {
        self.handle_canvas_pan_in_rect(ctx, self.canvas_rect(ctx));
    }

    #[profiling::function]
    pub(in super::super) fn handle_canvas_pan_in_rect(&mut self, ctx: &Context, canvas_rect: Rect) {
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
                input.smooth_scroll_delta + input.raw_scroll_delta,
                input.pointer.delta(),
                input.zoom_delta(),
            )
        });
        let pointer_in_canvas = pointer_position.is_some_and(|position| canvas_rect.contains(position));
        let space_drag_claimed =
            pointer_in_canvas && primary_down && space_down && space_drag_modifier_active(modifiers);
        let ctrl_or_cmd = modifiers.ctrl || modifiers.command;
        let pointer_over_terminal_body = primary_selection_routing_active()
            && pointer_position.is_some_and(|position| {
                self.terminal_body_screen_rects
                    .values()
                    .any(|rect| rect.contains(position))
            });
        let terminal_events = self.terminal_events_for_viewport(ctx.viewport_id(), &events);
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
                && self.panel_screen_rects.values().any(|rect| rect.contains(position))
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

    fn terminal_events_for_viewport(
        &mut self,
        viewport_id: egui::ViewportId,
        events: &[Event],
    ) -> Vec<TerminalInputEvent> {
        let frame_keyboard_events = self.frame_keyboard_events.remove(&viewport_id).unwrap_or_default();
        terminal_input_events(events, frame_keyboard_events)
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
    use super::{MiddlePanMode, MiddlePanTarget, next_middle_pan_active, primary_selection_routing_active};

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
