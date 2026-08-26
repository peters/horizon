//! Pointer capture, click replay, coordinate translation, and wheel routing.

use egui::{Event, PointerButton, Ui};
use horizon_core::browser::{BrowserButton, BrowserCommand, BrowserInput, BrowserModifiers, BrowserPanelState};

use super::keyboard::{key_modifiers, to_browser_modifiers};
use crate::browser_widget::{BrowserPointerClick, BrowserUiState};

/// CDP `buttons` bitmask for currently-down mouse buttons.
const BUTTON_LEFT: u32 = 1;
const BUTTON_RIGHT: u32 = 2;
const BUTTON_MIDDLE: u32 = 4;

/// The frame-level pointer context for one panel: the geometry this panel
/// owns, whether the frame-end pointer is over it, and the frame-wide
/// press hint used by the one-scan fast path.
#[derive(Clone, Copy)]
pub(super) struct PointerFrame {
    pub(super) rect: egui::Rect,
    pub(super) frame_size: [f32; 2],
    pub(super) pointer_target: bool,
    pub(super) frame_has_pointer_button: bool,
}

pub(super) fn events(
    ui: &Ui,
    events: &[Event],
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    frame: PointerFrame,
) {
    let ctx = ui.ctx();
    // Pointer positions arrive in global (window) space; the body rect is
    // in this layer's local space (panels are drawn on a transformed
    // canvas), so transform before hit-testing — same pattern as the
    // terminal widget's input.
    let from_global = ctx.layer_transform_from_global(ui.layer_id());
    let transform = |pos: egui::Pos2| from_global.map_or(pos, |t| t * pos);
    let frame_pos = ctx.input(|i| i.pointer.interact_pos());
    let frame_final_modifiers = key_modifiers(ui);
    // One-scan fast path: a panel that is not under the pointer, owns no
    // capture, and cannot own a press in this frame consumes nothing from
    // the event slice, so skip it (with N browser panels the event slice
    // would otherwise be scanned N times per pointer-heavy frame).
    if !frame.pointer_target && !has_pointer_capture(state) && !frame.frame_has_pointer_button {
        state.pointer_modifiers = frame_final_modifiers;
        return;
    }
    let event_time = ctx.input(|input| input.time);
    let click_thresholds = click_thresholds(ctx);
    // Frame-end pointer position: only used for wheel events, which carry
    // no per-event position.
    let wheel_pos = frame_pos
        .map(transform)
        .filter(|p| frame.pointer_target && frame.rect.contains(*p));

    // Replay this frame's pointer events with their own positions. A fast
    // gesture (press, moves, release) can be batched into a single
    // rendered frame; sampling only the frame-end position would collapse
    // the whole drag to one point. A panel only owns a press that started
    // inside its body, so a drag released over another panel (or the
    // canvas) still ends with exactly one release. Chrome coalesces adjacent
    // mouseMoved messages, so only the final move in each adjacent run is
    // forwarded while button and wheel ordering is preserved.
    let mut event_buttons = captured_buttons(state);
    let mut event_modifiers = state.pointer_modifiers;
    let mut pending_move: Option<(f64, f64, u32, BrowserModifiers)> = None;
    for event in events {
        if let Some(modifiers) = modifiers_for_event(event) {
            // A pending move precedes this modifier-bearing event, so emit it
            // with the state that was active when the move occurred.
            flush_pending_move(browser, &mut pending_move);
            event_modifiers = to_browser_modifiers(modifiers);
        }
        match event {
            Event::PointerButton {
                pos, button, pressed, ..
            } => {
                flush_pending_move(browser, &mut pending_move);
                replay_button_event(
                    browser,
                    state,
                    &mut event_buttons,
                    ButtonReplayContext {
                        frame,
                        event_time,
                        click_thresholds,
                    },
                    ButtonReplayEvent {
                        global_position: *pos,
                        button: *button,
                        pressed: *pressed,
                        modifiers: event_modifiers,
                    },
                    &transform,
                );
            }
            Event::PointerMoved(pos) => {
                handle_pointer_moved(
                    browser,
                    state,
                    frame,
                    transform(*pos),
                    &mut event_buttons,
                    event_modifiers,
                    &mut pending_move,
                );
            }
            // The pointer left the window mid-drag: end the drag where we
            // last saw it instead of stranding Chrome's button state down.
            Event::PointerGone => {
                flush_pending_move(browser, &mut pending_move);
                let p = state.last_mouse.unwrap_or_else(|| frame.rect.center());
                for captured in &mut state.captured_clicks {
                    let Some(click) = captured.take() else {
                        continue;
                    };
                    let (x, y) = to_page_coords(frame.rect, frame.frame_size, p);
                    event_buttons &= !button_mask(click.button);
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button: click.button,
                        click_count: click.count,
                        buttons: event_buttons,
                        modifiers: event_modifiers,
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
                    let (x, y) = to_page_coords(frame.rect, frame.frame_size, p);
                    let (delta_x, delta_y) = cdp_wheel_delta(*delta, scale);
                    browser.send(BrowserCommand::Input(BrowserInput::Wheel {
                        x,
                        y,
                        delta_x,
                        delta_y,
                        modifiers: event_modifiers,
                    }));
                }
            }
            _ => flush_pending_move(browser, &mut pending_move),
        }
    }
    flush_pending_move(browser, &mut pending_move);
    state.pointer_modifiers = frame_final_modifiers;
}

/// Replay one pointer-button press/release into the page, translating its
/// global position into this panel's page coordinates.
#[derive(Clone, Copy)]
struct ButtonReplayContext {
    frame: PointerFrame,
    event_time: f64,
    click_thresholds: ClickThresholds,
}

#[derive(Clone, Copy)]
struct ButtonReplayEvent {
    global_position: egui::Pos2,
    button: PointerButton,
    pressed: bool,
    modifiers: BrowserModifiers,
}

/// Track one pointer move for this panel, or — when the pointer leaves the
/// body without a capture — send a single clamped-edge move so the page's
/// mouseout/leave fires, then stop tracking.
fn handle_pointer_moved(
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    frame: PointerFrame,
    p: egui::Pos2,
    event_buttons: &mut u32,
    event_modifiers: BrowserModifiers,
    pending_move: &mut Option<(f64, f64, u32, BrowserModifiers)>,
) {
    let tracking = should_track_pointer(frame.pointer_target, frame.rect.contains(p), has_pointer_capture(state));
    if tracking {
        // Movement dedup: only forward real movement. `None` (the first move,
        // or the first after PointerGone) must be forwarded, or hover would
        // be dead until a click.
        if state.last_mouse.is_none_or(|last| (last - p).length() >= 0.5) {
            let (x, y) = to_page_coords(frame.rect, frame.frame_size, p);
            state.last_mouse = Some(p);
            *pending_move = Some((x, y, *event_buttons, event_modifiers));
        }
    } else if state.last_mouse.is_some() {
        let edge = egui::Pos2::new(
            p.x.clamp(frame.rect.min.x, frame.rect.max.x),
            p.y.clamp(frame.rect.min.y, frame.rect.max.y),
        );
        let (x, y) = to_page_coords(frame.rect, frame.frame_size, edge);
        browser.send(BrowserCommand::Input(BrowserInput::MouseMove {
            x,
            y,
            buttons: *event_buttons,
            modifiers: event_modifiers,
        }));
        state.last_mouse = None;
    }
}

fn replay_button_event(
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    event_buttons: &mut u32,
    context: ButtonReplayContext,
    event: ButtonReplayEvent,
    transform: &dyn Fn(egui::Pos2) -> egui::Pos2,
) {
    let Some(browser_button) = egui_button(event.button) else {
        return;
    };
    let p = transform(event.global_position);
    replay_pointer_button(
        browser,
        state,
        event_buttons,
        PointerButtonReplay {
            global_position: event.global_position,
            local_position: p,
            button: browser_button,
            pressed: event.pressed,
            pointer_target: context.frame.pointer_target,
            event_time: context.event_time,
            max_click_dist: context.click_thresholds.distance,
            max_click_duration: context.click_thresholds.duration,
            max_double_click_delay: context.click_thresholds.double_click_delay,
            modifiers: event.modifiers,
            rect: context.frame.rect,
            frame_size: context.frame.frame_size,
        },
    );
}

#[derive(Clone, Copy)]
struct ClickThresholds {
    distance: f32,
    duration: f64,
    double_click_delay: f64,
}

fn click_thresholds(ctx: &egui::Context) -> ClickThresholds {
    ctx.options(|options| {
        let input = &options.input_options;
        ClickThresholds {
            distance: input.max_click_dist,
            duration: input.max_click_duration,
            double_click_delay: input.max_double_click_delay,
        }
    })
}

fn modifiers_for_event(event: &Event) -> Option<egui::Modifiers> {
    match event {
        Event::Key { modifiers, .. } | Event::PointerButton { modifiers, .. } | Event::MouseWheel { modifiers, .. } => {
            Some(*modifiers)
        }
        _ => None,
    }
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
    fn pointer_events_keep_their_modifier_snapshots() {
        let shift_click = Event::PointerButton {
            pos: egui::Pos2::ZERO,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::SHIFT,
        };
        let later_unmodified_key = Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };

        assert_eq!(modifiers_for_event(&shift_click), Some(egui::Modifiers::SHIFT));
        assert_eq!(modifiers_for_event(&later_unmodified_key), Some(egui::Modifiers::NONE));
        assert!(to_browser_modifiers(egui::Modifiers::SHIFT).shift);
        assert!(!to_browser_modifiers(egui::Modifiers::NONE).shift);
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
    fn button_masks_follow_the_cdp_buttons_bitfield() {
        assert_eq!(button_mask(BrowserButton::Left), 1);
        assert_eq!(button_mask(BrowserButton::Right), 2);
        assert_eq!(button_mask(BrowserButton::Middle), 4);
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
