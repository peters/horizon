use std::mem;

use alacritty_terminal::term::TermMode;
use egui::{Context, Event, Key, Modifiers, Rect, Vec2};
use horizon_core::{Panel, PanelId, WorkspaceId};

use super::super::super::input::{GridPoint, TerminalInputEvent, WheelAction, terminal_input_events, wheel_action};
use super::super::canvas_drag::canvas_drag_delta;
use super::super::canvas_scroll::{
    ScrollTarget, WheelStep, canvas_zoom_delta, reset_canvas_scroll, route_canvas_scroll,
};
use super::super::shortcuts::shortcut_pressed;
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

impl HorizonApp {
    pub(in super::super) fn handle_fullscreen_toggle(&mut self, ctx: &Context) {
        if self.host_dialog_open()
            || super::super::panels::session_picker_panel(ctx).is_some_and(|panel| self.board.panel(panel).is_some())
        {
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
            self.consume_navigation_key(ctx, self.shortcuts.fullscreen_window);
            let is_fullscreen = ctx.input(|input| input.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
        } else if panel_toggle {
            self.consume_navigation_key(ctx, self.shortcuts.fullscreen_panel);
            self.fullscreen_panel = if self.fullscreen_panel.is_some() {
                None
            } else {
                self.board.focused
            };
        } else if exit_fullscreen && self.fullscreen_panel.is_some() && self.command_palette.is_none() {
            self.consume_navigation_key(ctx, self.shortcuts.exit_fullscreen_panel);
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
        if self.host_dialog_open()
            || super::super::panels::session_picker_panel(ctx).is_some_and(|panel| self.board.panel(panel).is_some())
            || self.preset_picker_rect(ctx).is_some_and(|rect| {
                ctx.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .is_some_and(|position| rect.contains(position))
                })
            })
        {
            reset_canvas_scroll(ctx);
            self.terminal_keyboard_events.clear();
            self.frame_keyboard_events.remove(&ctx.viewport_id());
            return;
        }
        let (events, pointer_position, middle_down, primary_down, space_down, modifiers, pointer_delta) =
            ctx.input(|input| {
                (
                    input.events.clone(),
                    input.pointer.interact_pos().or_else(|| input.pointer.hover_pos()),
                    input.pointer.middle_down(),
                    input.pointer.primary_down(),
                    input.key_down(egui::Key::Space),
                    input.modifiers,
                    input.pointer.delta(),
                )
            });
        let panel_geometry = self.visible_panel_geometry_for_canvas_view(canvas_rect, visible_workspace);
        let pointer_in_canvas = pointer_position.is_some_and(|position| {
            canvas_rect.contains(position)
                && !(visible_workspace.is_none()
                    && self
                        .work_resume_overlay_rect(ctx)
                        .is_some_and(|rect| rect.contains(position)))
        });
        let zoom_delta = canvas_zoom_delta(ctx, pointer_in_canvas);
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
        let primary_canvas_drag = canvas_drag_delta(
            ctx,
            canvas_rect,
            &panel_geometry.iter().map(|(_, geometry)| geometry.screen_rect),
            &events,
        );
        self.canvas_pan_input_claimed =
            pointer_in_canvas && (self.middle_pan_active || space_drag_claimed || primary_canvas_drag.is_some());
        // Zoom and pan come from different wheel events, so a frame can do
        // both: plain wheels still route below.
        if pointer_in_canvas && (zoom_delta - 1.0).abs() > f32::EPSILON {
            let anchor = pointer_position.unwrap_or_else(|| canvas_rect.center());
            if self.zoom_canvas_at(canvas_rect, anchor, self.canvas_view.zoom * zoom_delta) {
                self.clear_terminal_selections();
            }
            self.canvas_pan_input_claimed = false;
        }

        let drag_panning = self.canvas_pan_input_claimed;
        // Focused panels are Foreground. Other overlapping panels follow
        // egui's persisted Middle-layer order, with board order as a fallback
        // before egui has registered their areas.
        let panel_under_pointer = pointer_position.and_then(|position| {
            let hits = |id: PanelId| {
                panel_geometry
                    .iter()
                    .any(|(candidate, geometry)| *candidate == id && geometry.screen_rect.contains(position))
            };
            self.board.focused.filter(|id| hits(*id)).or_else(|| {
                if let Some(layer) = ctx.layer_id_at(position)
                    && let Some((id, _)) = panel_geometry.iter().find(|(id, geometry)| {
                        layer.id == egui::Id::unique(("panel", id.0)) && geometry.screen_rect.contains(position)
                    })
                {
                    return Some(*id);
                }
                panel_geometry
                    .iter()
                    .rev()
                    .find(|(_, geometry)| geometry.screen_rect.contains(position))
                    .map(|(id, _)| *id)
            })
        });
        let scroll_target = panel_under_pointer.map_or(ScrollTarget::Canvas, ScrollTarget::Panel);
        let terminal_under_pointer = panel_under_pointer.filter(|panel| {
            panel_geometry.iter().any(|(id, geometry)| {
                id == panel
                    && geometry
                        .terminal_body_screen_rect
                        .is_some_and(|body| pointer_position.is_some_and(|position| body.contains(position)))
            })
        });
        let cell_size = crate::terminal_widget::wheel_cell_size(ctx);
        let mut pending_scrollback = None;
        let mut scroll_routing =
            route_canvas_scroll(ctx, scroll_target, pointer_in_canvas && !drag_panning, |panel, step| {
                // A latched owner outside the pointer's terminal body will
                // not receive this event, so earlier events cannot move its
                // predicted scrollback either.
                if terminal_under_pointer != Some(panel) {
                    pending_scrollback = None;
                }
                self.panel_scroll_exhausted(panel, step, cell_size, &mut pending_scrollback)
            });
        scroll_routing.discard_displaced_wheels(scroll_target);
        scroll_routing.defer_for_panel_delivery(ctx);
        let pan_delta = if drag_panning {
            primary_canvas_drag.unwrap_or(pointer_delta)
        } else if scroll_routing.pans_canvas {
            scroll_routing.pan
        } else {
            Vec2::ZERO
        };

        self.is_panning = pan_delta != Vec2::ZERO;
        self.canvas_pan_input_claimed |= scroll_routing.pans_canvas;
        scroll_routing.consume(ctx, &mut self.terminal_keyboard_events);
        if self.is_panning {
            self.pan_target = None;
            let mut pan_offset = Vec2::new(self.canvas_view.pan_offset[0], self.canvas_view.pan_offset[1]);
            pan_offset += pan_delta;
            self.canvas_view.set_pan_offset([pan_offset.x, pan_offset.y]);
            self.mark_runtime_dirty();
            self.clear_terminal_selections();
        }
    }

    /// Whether `panel` has run out of scroll to absorb one wheel event. A
    /// latched gesture chains to the canvas once its own panel cannot absorb
    /// any more, the way a browser hands off at a scroll container's edge.
    /// Only terminals expose a reliable extent; other panel kinds keep the
    /// gesture.
    ///
    /// The terminal applies a frame's wheel events only after routing, so
    /// `pending` carries the scrollback position the events already judged
    /// will leave it at, advanced with the terminal's own step conversion.
    pub(in crate::app) fn panel_scroll_exhausted(
        &self,
        panel: PanelId,
        step: WheelStep,
        cell_size: Vec2,
        pending: &mut Option<(PanelId, usize)>,
    ) -> bool {
        let Some(terminal) = self
            .board
            .panels
            .iter()
            .find(|candidate| candidate.id == panel)
            .and_then(Panel::terminal)
        else {
            return false;
        };
        let mode = terminal.mode();
        if mode.intersects(TermMode::MOUSE_MODE) || mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            return false;
        }
        let history = terminal.history_size();
        let position = pending
            .filter(|(owner, _)| *owner == panel)
            .map_or_else(|| terminal.scrollback(), |(_, position)| position);
        let exhausted = if step.delta.y > 0.0 {
            position >= history
        } else if step.delta.y < 0.0 {
            position == 0
        } else {
            // Horizontal-only swipes have nothing to absorb in the scrollback.
            true
        };
        if !exhausted {
            let point = GridPoint { line: 0, column: 0 };
            let lines = match wheel_action(step.delta, step.unit, cell_size, step.modifiers, mode, point) {
                Some(WheelAction::Scrollback(lines)) => lines,
                Some(WheelAction::Pty(_)) | None => 0,
            };
            let moved = i64::try_from(position)
                .unwrap_or(i64::MAX)
                .saturating_add(i64::from(lines));
            let history = i64::try_from(history).unwrap_or(i64::MAX);
            *pending = Some((panel, usize::try_from(moved.clamp(0, history)).unwrap_or(0)));
        }
        exhausted
    }

    fn clear_terminal_selections(&self) {
        for panel in &self.board.panels {
            if let Some(terminal) = panel.terminal() {
                terminal.clear_selection();
            }
        }
    }

    pub(in crate::app) fn terminal_events_for_viewport(
        &mut self,
        ctx: &Context,
        events: &[Event],
    ) -> Vec<TerminalInputEvent> {
        let viewport_id = ctx.viewport_id();
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
