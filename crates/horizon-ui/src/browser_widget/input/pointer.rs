//! Pointer capture, click replay, coordinate translation, and wheel routing.

use egui::{Event, PointerButton, Ui};
use horizon_core::browser::{BrowserButton, BrowserCommand, BrowserInput, BrowserModifiers, BrowserPanelState};

use super::keyboard::key_modifiers;
use crate::browser_widget::{BrowserPointerClick, BrowserUiState};

/// CDP `buttons` bitmask for currently-down mouse buttons.
const BUTTON_LEFT: u32 = 1;
const BUTTON_MIDDLE: u32 = 2;
const BUTTON_RIGHT: u32 = 4;

pub(super) fn events(
    ui: &Ui,
    events: &[Event],
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    rect: egui::Rect,
    frame_size: [f32; 2],
    pointer_target: bool,
) {
    let ctx = ui.ctx();
    // Pointer positions arrive in global (window) space; the body rect is
    // in this layer's local space (panels are drawn on a transformed
    // canvas), so transform before hit-testing — same pattern as the
    // terminal widget's input.
    let from_global = ctx.layer_transform_from_global(ui.layer_id());
    let transform = |pos: egui::Pos2| from_global.map_or(pos, |t| t * pos);
    let frame_pos = ctx.input(|i| i.pointer.interact_pos());
    let modifiers = key_modifiers(ui);
    let event_time = ctx.input(|input| input.time);
    let (max_click_dist, max_click_duration, max_double_click_delay) = ctx.options(|options| {
        let input = &options.input_options;
        (
            input.max_click_dist,
            input.max_click_duration,
            input.max_double_click_delay,
        )
    });
    // Frame-end pointer position: only used for wheel events, which carry
    // no per-event position.
    let wheel_pos = frame_pos.map(transform).filter(|p| pointer_target && rect.contains(*p));

    // Replay this frame's pointer events with their own positions. A fast
    // gesture (press, moves, release) can be batched into a single
    // rendered frame; sampling only the frame-end position would collapse
    // the whole drag to one point. A panel only owns a press that started
    // inside its body, so a drag released over another panel (or the
    // canvas) still ends with exactly one release. Chrome coalesces adjacent
    // mouseMoved messages, so only the final move in each adjacent run is
    // forwarded while button and wheel ordering is preserved.
    let mut event_buttons = captured_buttons(state);
    let mut pending_move: Option<(f64, f64, u32, BrowserModifiers)> = None;
    for event in events {
        match event {
            Event::PointerButton {
                pos, button, pressed, ..
            } => {
                flush_pending_move(browser, &mut pending_move);
                let Some(browser_button) = egui_button(*button) else {
                    continue;
                };
                let p = transform(*pos);
                replay_pointer_button(
                    browser,
                    state,
                    &mut event_buttons,
                    PointerButtonReplay {
                        global_position: *pos,
                        local_position: p,
                        button: browser_button,
                        pressed: *pressed,
                        pointer_target,
                        event_time,
                        max_click_dist,
                        max_click_duration,
                        max_double_click_delay,
                        modifiers,
                        rect,
                        frame_size,
                    },
                );
            }
            Event::PointerMoved(pos) => {
                let p = transform(*pos);
                let tracking = should_track_pointer(pointer_target, rect.contains(p), has_pointer_capture(state));
                // Movement dedup: only forward real movement. `None` (the
                // first move, or the first after PointerGone) must be
                // forwarded, or hover would be dead until a click.
                if tracking && state.last_mouse.is_none_or(|last| (last - p).length() >= 0.5) {
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    state.last_mouse = Some(p);
                    pending_move = Some((x, y, event_buttons, modifiers));
                }
            }
            // The pointer left the window mid-drag: end the drag where we
            // last saw it instead of stranding Chrome's button state down.
            Event::PointerGone => {
                flush_pending_move(browser, &mut pending_move);
                let p = state.last_mouse.unwrap_or_else(|| rect.center());
                for captured in &mut state.captured_clicks {
                    let Some(click) = captured.take() else {
                        continue;
                    };
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    event_buttons &= !button_mask(click.button);
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button: click.button,
                        click_count: click.count,
                        buttons: event_buttons,
                        modifiers,
                    }));
                }
                state.last_mouse = None;
            }
            Event::MouseWheel { unit, delta, .. } => {
                flush_pending_move(browser, &mut pending_move);
                if let Some(p) = wheel_pos {
                    let scale = match *unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 16.0,
                        egui::MouseWheelUnit::Page => 500.0,
                    };
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    let (delta_x, delta_y) = cdp_wheel_delta(*delta, scale);
                    browser.send(BrowserCommand::Input(BrowserInput::Wheel {
                        x,
                        y,
                        delta_x,
                        delta_y,
                        modifiers,
                    }));
                }
            }
            _ => flush_pending_move(browser, &mut pending_move),
        }
    }
    flush_pending_move(browser, &mut pending_move);
}

#[derive(Clone, Copy)]
struct PointerButtonReplay {
    global_position: egui::Pos2,
    local_position: egui::Pos2,
    button: BrowserButton,
    pressed: bool,
    pointer_target: bool,
    event_time: f64,
    max_click_dist: f32,
    max_click_duration: f64,
    max_double_click_delay: f64,
    modifiers: BrowserModifiers,
    rect: egui::Rect,
    frame_size: [f32; 2],
}

fn replay_pointer_button(
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    event_buttons: &mut u32,
    event: PointerButtonReplay,
) {
    let (x, y) = to_page_coords(event.rect, event.frame_size, event.local_position);
    if event.pressed {
        if !event.pointer_target || !event.rect.contains(event.local_position) {
            return;
        }
        let click_count = next_click_count(
            state.last_click,
            event.button,
            event.global_position,
            event.event_time,
            event.max_double_click_delay,
            event.max_click_dist,
        );
        state.captured_clicks[button_index(event.button)] = Some(BrowserPointerClick {
            button: event.button,
            position: event.global_position,
            time: event.event_time,
            count: click_count,
        });
        state.last_mouse = Some(event.local_position);
        *event_buttons |= button_mask(event.button);
        browser.send(BrowserCommand::Input(BrowserInput::MousePress {
            x,
            y,
            button: event.button,
            click_count,
            buttons: *event_buttons,
            modifiers: event.modifiers,
        }));
        return;
    }

    let Some(click) = state.captured_clicks[button_index(event.button)].take() else {
        return;
    };
    state.last_mouse = Some(event.local_position);
    *event_buttons &= !button_mask(event.button);
    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
        x,
        y,
        button: event.button,
        click_count: click.count,
        buttons: *event_buttons,
        modifiers: event.modifiers,
    }));
    if event.global_position.distance(click.position) <= event.max_click_dist
        && event.event_time - click.time <= event.max_click_duration
    {
        state.last_click = Some(BrowserPointerClick {
            position: event.global_position,
            time: event.event_time,
            ..click
        });
    } else {
        state.last_click = None;
    }
}

fn flush_pending_move(browser: &BrowserPanelState, pending_move: &mut Option<(f64, f64, u32, BrowserModifiers)>) {
    if let Some((x, y, buttons, modifiers)) = pending_move.take() {
        browser.send(BrowserCommand::Input(BrowserInput::MouseMove {
            x,
            y,
            buttons,
            modifiers,
        }));
    }
}

pub(in crate::browser_widget) fn cancel_pointer_capture(
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    rect: Option<egui::Rect>,
    frame_size: Option<[f32; 2]>,
) {
    let position = state.last_mouse.or_else(|| rect.map(|rect| rect.center()));
    let coordinates = rect
        .zip(frame_size)
        .zip(position)
        .map_or((0.0, 0.0), |((rect, frame_size), position)| {
            to_page_coords(rect, frame_size, position)
        });
    let mut buttons = captured_buttons(state);
    for captured in &mut state.captured_clicks {
        let Some(click) = captured.take() else {
            continue;
        };
        buttons &= !button_mask(click.button);
        browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
            x: coordinates.0,
            y: coordinates.1,
            button: click.button,
            click_count: click.count,
            buttons,
            modifiers: BrowserModifiers::none(),
        }));
    }
    state.last_mouse = None;
}

fn captured_buttons(state: &BrowserUiState) -> u32 {
    state
        .captured_clicks
        .iter()
        .flatten()
        .fold(0, |buttons, click| buttons | button_mask(click.button))
}

fn has_pointer_capture(state: &BrowserUiState) -> bool {
    state.captured_clicks.iter().any(Option::is_some)
}

const fn button_index(button: BrowserButton) -> usize {
    match button {
        BrowserButton::Left => 0,
        BrowserButton::Middle => 1,
        BrowserButton::Right => 2,
    }
}

const fn should_track_pointer(pointer_target: bool, inside_rect: bool, captured: bool) -> bool {
    (pointer_target && inside_rect) || captured
}

fn next_click_count(
    last_click: Option<BrowserPointerClick>,
    button: BrowserButton,
    position: egui::Pos2,
    time: f64,
    max_delay: f64,
    max_distance: f32,
) -> u32 {
    last_click
        .filter(|last| {
            last.button == button
                && time >= last.time
                && time - last.time <= max_delay
                && position.distance(last.position) <= max_distance
        })
        .map_or(1, |last| last.count.saturating_add(1).min(3))
}

/// egui mouse button → CDP button (mouse-only; touch is not forwarded).
fn egui_button(button: PointerButton) -> Option<BrowserButton> {
    match button {
        PointerButton::Primary => Some(BrowserButton::Left),
        PointerButton::Middle => Some(BrowserButton::Middle),
        PointerButton::Secondary => Some(BrowserButton::Right),
        _ => None,
    }
}

fn to_page_coords(rect: egui::Rect, frame_size: [f32; 2], pos: egui::Pos2) -> (f64, f64) {
    let x = f64::from(((pos.x - rect.min.x) / rect.width() * frame_size[0]).clamp(0.0, frame_size[0]));
    let y = f64::from(((pos.y - rect.min.y) / rect.height() * frame_size[1]).clamp(0.0, frame_size[1]));
    (x, y)
}

fn cdp_wheel_delta(delta: egui::Vec2, scale: f32) -> (f64, f64) {
    // egui reports positive movement toward the top/left; CDP uses the DOM
    // wheel convention where positive deltas move toward the bottom/right.
    (-f64::from(delta.x * scale), -f64::from(delta.y * scale))
}

fn button_mask(button: BrowserButton) -> u32 {
    match button {
        BrowserButton::Left => BUTTON_LEFT,
        BrowserButton::Middle => BUTTON_MIDDLE,
        BrowserButton::Right => BUTTON_RIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_panel_ignores_pointer_until_it_owns_capture() {
        assert!(!should_track_pointer(false, true, false));
        assert!(should_track_pointer(true, true, false));
        assert!(should_track_pointer(false, false, true));
    }

    #[test]
    fn wheel_deltas_follow_the_dom_direction() {
        assert_eq!(cdp_wheel_delta(egui::vec2(2.0, 3.0), 16.0), (-32.0, -48.0));
        assert_eq!(cdp_wheel_delta(egui::vec2(-2.0, -3.0), 16.0), (32.0, 48.0));
    }

    #[test]
    fn simultaneous_button_captures_keep_each_button_down() {
        let mut state = BrowserUiState::default();
        state.captured_clicks[button_index(BrowserButton::Left)] = Some(BrowserPointerClick {
            button: BrowserButton::Left,
            position: egui::Pos2::ZERO,
            time: 1.0,
            count: 1,
        });
        state.captured_clicks[button_index(BrowserButton::Right)] = Some(BrowserPointerClick {
            button: BrowserButton::Right,
            position: egui::Pos2::ZERO,
            time: 1.1,
            count: 1,
        });

        assert_eq!(captured_buttons(&state), BUTTON_LEFT | BUTTON_RIGHT);
        assert!(has_pointer_capture(&state));
        let _ = state.captured_clicks[button_index(BrowserButton::Left)].take();
        assert_eq!(captured_buttons(&state), BUTTON_RIGHT);
    }

    #[test]
    fn click_sequence_tracks_button_time_and_position() {
        let first = BrowserPointerClick {
            button: BrowserButton::Left,
            position: egui::pos2(20.0, 30.0),
            time: 10.0,
            count: 1,
        };

        assert_eq!(
            next_click_count(Some(first), BrowserButton::Left, egui::pos2(22.0, 31.0), 10.2, 0.3, 6.0,),
            2
        );
        assert_eq!(
            next_click_count(
                Some(first),
                BrowserButton::Right,
                egui::pos2(22.0, 31.0),
                10.2,
                0.3,
                6.0,
            ),
            1
        );
        assert_eq!(
            next_click_count(Some(first), BrowserButton::Left, egui::pos2(40.0, 30.0), 10.2, 0.3, 6.0,),
            1
        );
        assert_eq!(
            next_click_count(Some(first), BrowserButton::Left, egui::pos2(20.0, 30.0), 10.4, 0.3, 6.0,),
            1
        );
    }
}
