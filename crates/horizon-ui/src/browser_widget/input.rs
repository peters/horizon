//! egui → CDP input translation for the focused browser panel.

use egui::{Event, Key, Modifiers, PointerButton, Ui};
use horizon_core::AppShortcuts;
use horizon_core::browser::{
    BrowserButton, BrowserCommand, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers, BrowserPanelState,
};

use crate::browser_widget::{BrowserPointerClick, BrowserUiState};

/// CDP `buttons` bitmask for currently-down mouse buttons.
const BUTTON_LEFT: u32 = 1;
const BUTTON_MIDDLE: u32 = 2;
const BUTTON_RIGHT: u32 = 4;

/// Focus/interaction flags for one frame.
#[derive(Clone, Copy)]
pub(crate) struct InputFlags<'a> {
    pub(crate) events: &'a [Event],
    pub(crate) is_focused: bool,
    pub(crate) interactive: bool,
    pub(crate) pointer_viewport: PointerViewportState,
    pub(crate) url_focused: bool,
    pub(crate) shortcuts: &'a AppShortcuts,
    pub(crate) exit_fullscreen_shortcut_owner: ShortcutOwner,
}

#[derive(Clone, Copy)]
pub(crate) enum PointerViewportState {
    AwaitingFrame,
    Ready,
}

#[derive(Clone, Copy)]
pub(crate) enum ShortcutOwner {
    App,
    Page,
}

pub fn handle(
    ui: &mut Ui,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    body: Option<egui::Rect>,
    frame_size: Option<[f32; 2]>,
    pointer_target: bool,
    flags: InputFlags<'_>,
) {
    let page_keyboard_active = flags.interactive && flags.is_focused && !flags.url_focused;
    if !page_keyboard_active {
        release_pressed_keys(browser, state, key_modifiers(ui));
    }
    if !flags.interactive {
        return;
    }
    if matches!(flags.pointer_viewport, PointerViewportState::Ready)
        && let (Some(rect), Some(frame_size)) = (body, frame_size)
    {
        pointer_events(ui, flags.events, browser, state, rect, frame_size, pointer_target);
    }
    if flags.is_focused {
        let exit_fullscreen_shortcut_active = matches!(flags.exit_fullscreen_shortcut_owner, ShortcutOwner::App);
        if flags.url_focused {
            browser_shortcut_events(flags.events, browser, state);
        } else {
            keyboard_events(
                ui,
                flags.events,
                browser,
                state,
                flags.shortcuts,
                exit_fullscreen_shortcut_active,
            );
        }
    }
}

fn pointer_events(
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
                    browser.send(BrowserCommand::Input(BrowserInput::Wheel {
                        x,
                        y,
                        delta_x: f64::from(delta.x * scale),
                        delta_y: f64::from(delta.y * scale),
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

pub(super) fn cancel_pointer_capture(
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

fn keyboard_events(
    ui: &Ui,
    events: &[Event],
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    shortcuts: &AppShortcuts,
    exit_fullscreen_shortcut_active: bool,
) {
    let KeyboardFrameEvents {
        key_texts,
        mut key_chars,
        mut shortcut_chars,
        shortcut_presses,
    } = collect_keyboard_frame_events(events, shortcuts, exit_fullscreen_shortcut_active);
    for (event_index, event) in events.iter().enumerate() {
        match event {
            Event::Text(text) if !text.is_empty() => {
                let shortcut_text = take_matching_single_char(text, &mut shortcut_chars);
                let duplicate_key_text = take_matching_single_char(text, &mut key_chars);
                if !shortcut_text && !duplicate_key_text {
                    browser.send(BrowserCommand::Input(BrowserInput::InsertText { text: text.clone() }));
                }
            }
            // egui_winit turns Ctrl/Cmd+V into a global Paste event (the URL
            // bar consumes its own copy while focused). Both paste and IME
            // commits reach the page as one CDP insertText operation.
            Event::Ime(egui::ImeEvent::Commit(text)) | Event::Paste(text) if !text.is_empty() => {
                browser.send(BrowserCommand::Input(BrowserInput::InsertText { text: text.clone() }));
            }
            Event::Copy => send_clipboard_shortcut(
                browser,
                state,
                BrowserKey::Char('c'),
                BrowserEditCommand::Copy,
                clipboard_modifiers(ui),
            ),
            Event::Cut => send_clipboard_shortcut(
                browser,
                state,
                BrowserKey::Char('x'),
                BrowserEditCommand::Cut,
                clipboard_modifiers(ui),
            ),
            Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => handle_key_event(
                browser,
                state,
                BrowserKeyEvent {
                    key: *key,
                    pressed: *pressed,
                    repeat: *repeat,
                    modifiers: *modifiers,
                    key_text: key_texts[event_index],
                    app_shortcut: shortcut_presses[event_index],
                },
            ),
            _ => {}
        }
    }
}

fn browser_shortcut_events(events: &[Event], browser: &BrowserPanelState, state: &mut BrowserUiState) {
    for event in events {
        let Event::Key {
            key,
            pressed,
            repeat,
            modifiers,
            ..
        } = event
        else {
            continue;
        };
        if should_route_key_while_url_focused(state, *key, *modifiers) {
            handle_key_event(
                browser,
                state,
                BrowserKeyEvent {
                    key: *key,
                    pressed: *pressed,
                    repeat: *repeat,
                    modifiers: *modifiers,
                    key_text: None,
                    app_shortcut: false,
                },
            );
        }
    }
}

fn should_route_key_while_url_focused(state: &BrowserUiState, key: Key, modifiers: Modifiers) -> bool {
    if is_reload_shortcut(key, modifiers) || (key == Key::Enter && state.url_submit_enter_pending) {
        return true;
    }
    key_to_browser_key(key)
        .is_some_and(|key| state.suppressed_shortcut_keys.contains(&key) || state.clipboard_release_keys.contains(&key))
}

fn release_pressed_keys(browser: &BrowserPanelState, state: &mut BrowserUiState, modifiers: BrowserModifiers) {
    for (key, text) in state.pressed_keys.drain() {
        browser.send(BrowserCommand::Input(BrowserInput::KeyUp { key, text, modifiers }));
    }
}

fn send_clipboard_shortcut(
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    key: BrowserKey,
    edit_command: BrowserEditCommand,
    modifiers: BrowserModifiers,
) {
    state.clipboard_release_keys.insert(key);
    state.pressed_keys.remove(&key);
    browser.send(BrowserCommand::Input(BrowserInput::KeyDown {
        key,
        text: None,
        modifiers,
        repeat: false,
        edit_command: Some(edit_command),
    }));
    browser.send(BrowserCommand::Input(BrowserInput::KeyUp {
        key,
        text: None,
        modifiers,
    }));
}

struct KeyboardFrameEvents {
    key_texts: Vec<Option<char>>,
    key_chars: Vec<char>,
    shortcut_chars: Vec<char>,
    shortcut_presses: Vec<bool>,
}

fn collect_keyboard_frame_events(
    events: &[Event],
    shortcuts: &AppShortcuts,
    exit_fullscreen_shortcut_active: bool,
) -> KeyboardFrameEvents {
    let shortcut_presses: Vec<bool> = events
        .iter()
        .map(|event| app_shortcut_press(event, shortcuts, exit_fullscreen_shortcut_active))
        .collect();
    // Pair each printable key-down with its adjacent committed text. That text
    // is layout-authoritative (unlike a US-layout Shift prediction) and is
    // also removed from the later Event::Text path to avoid double insertion.
    let key_texts: Vec<Option<char>> = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            if shortcut_presses[index] {
                return None;
            }
            if let Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } = event
                && *pressed
                && !(modifiers.ctrl || modifiers.mac_cmd || modifiers.alt)
                && let Some(browser_key) = key_to_browser_key(*key)
            {
                committed_text_after_key(events, index)
                    .or_else(|| browser_key.printable_char_with_shift(modifiers.shift))
            } else {
                None
            }
        })
        .collect();
    let key_chars = key_texts.iter().flatten().copied().collect();
    let shortcut_chars = shortcut_presses
        .iter()
        .enumerate()
        .filter(|(_, suppressed)| **suppressed)
        .filter_map(|(index, _)| committed_text_after_key(events, index))
        .collect();
    KeyboardFrameEvents {
        key_texts,
        key_chars,
        shortcut_chars,
        shortcut_presses,
    }
}

fn take_matching_single_char(text: &str, candidates: &mut Vec<char>) -> bool {
    (text.chars().count() == 1)
        .then(|| text.chars().next())
        .flatten()
        .and_then(|character| candidates.iter().position(|candidate| *candidate == character))
        .is_some_and(|index| {
            candidates.swap_remove(index);
            true
        })
}

#[derive(Clone, Copy)]
struct BrowserKeyEvent {
    key: Key,
    pressed: bool,
    repeat: bool,
    modifiers: Modifiers,
    key_text: Option<char>,
    app_shortcut: bool,
}

fn handle_key_event(browser: &BrowserPanelState, state: &mut BrowserUiState, event: BrowserKeyEvent) {
    let BrowserKeyEvent {
        key,
        pressed,
        repeat,
        modifiers,
        key_text,
        app_shortcut,
    } = event;
    if key == Key::Enter && state.url_submit_enter_pending {
        if !pressed {
            state.url_submit_enter_pending = false;
        }
        return;
    }
    let reload_shortcut = is_reload_shortcut(key, modifiers);
    let browser_key = key_to_browser_key(key);
    if consume_clipboard_release(state, browser_key, pressed) {
        return;
    }
    if app_shortcut {
        if let Some(browser_key) = browser_key {
            state.suppressed_shortcut_keys.insert(browser_key);
            state.pressed_keys.remove(&browser_key);
        }
        return;
    }
    if consume_suppressed_key(state, browser_key, pressed) {
        return;
    }
    if reload_shortcut {
        if pressed {
            if let Some(browser_key) = browser_key {
                // Modifiers can be released before the key. Remember the
                // consumed press so that later bare key-up cannot reach the
                // page without a matching key-down.
                state.suppressed_shortcut_keys.insert(browser_key);
                state.pressed_keys.remove(&browser_key);
            }
            // Consume every repeat in a held shortcut, but reload only once.
            if !repeat {
                browser.send(BrowserCommand::Reload);
            }
        }
        return;
    }
    if !pressed {
        if let Some(browser_key) = browser_key {
            let Some(text) = state.pressed_keys.remove(&browser_key) else {
                return;
            };
            browser.send(BrowserCommand::Input(BrowserInput::KeyUp {
                key: browser_key,
                text,
                modifiers: to_browser_modifiers(modifiers),
            }));
        }
        return;
    }
    let Some(browser_key) = browser_key else {
        return;
    };
    // A repeat can arrive after focus loss synthesized the matching key-up.
    // Do not start a new page key sequence with an orphan repeat; the next
    // physical non-repeat press remains eligible normally.
    if repeat && !state.pressed_keys.contains_key(&browser_key) {
        return;
    }
    let modifiers = to_browser_modifiers(modifiers);
    let edit_command = editing_command(key, modifiers);
    let text = key_text
        .map(|character| character.to_string())
        .filter(|_| !(modifiers.ctrl || modifiers.meta || modifiers.alt));
    state.pressed_keys.insert(browser_key, text.clone());
    browser.send(BrowserCommand::Input(BrowserInput::KeyDown {
        key: browser_key,
        text,
        modifiers,
        repeat,
        edit_command,
    }));
}

fn is_reload_shortcut(key: Key, modifiers: Modifiers) -> bool {
    key == Key::F5 || (key == Key::R && (modifiers.ctrl || modifiers.mac_cmd))
}

fn editing_command(key: Key, modifiers: BrowserModifiers) -> Option<BrowserEditCommand> {
    let primary = if cfg!(target_os = "macos") {
        modifiers.meta
    } else {
        modifiers.ctrl
    };
    (primary && !modifiers.alt && !modifiers.shift && key == Key::A).then_some(BrowserEditCommand::SelectAll)
}

fn consume_clipboard_release(state: &mut BrowserUiState, key: Option<BrowserKey>, pressed: bool) -> bool {
    let Some(key) = key.filter(|key| state.clipboard_release_keys.contains(key)) else {
        return false;
    };
    state.clipboard_release_keys.remove(&key);
    !pressed
}

fn consume_suppressed_key(state: &mut BrowserUiState, key: Option<BrowserKey>, pressed: bool) -> bool {
    let Some(key) = key.filter(|key| state.suppressed_shortcut_keys.contains(key)) else {
        return false;
    };
    if !pressed {
        state.suppressed_shortcut_keys.remove(&key);
        state.pressed_keys.remove(&key);
    }
    true
}

fn committed_text_after_key(events: &[Event], key_index: usize) -> Option<char> {
    for event in &events[key_index + 1..] {
        match event {
            Event::Text(text) if text.chars().count() == 1 => return text.chars().next(),
            Event::Text(_) | Event::Key { .. } | Event::Ime(egui::ImeEvent::Commit(_)) => return None,
            _ => {}
        }
    }
    None
}

fn app_shortcut_press(event: &Event, shortcuts: &AppShortcuts, exit_fullscreen_shortcut_active: bool) -> bool {
    crate::app::global_shortcut_bindings(shortcuts)
        .into_iter()
        .filter(|binding| *binding != shortcuts.save_editor)
        .filter(|binding| exit_fullscreen_shortcut_active || *binding != shortcuts.exit_fullscreen_panel)
        .any(|binding| crate::app::shortcuts::shortcut_event_matches(event, binding))
}

fn to_browser_modifiers(modifiers: Modifiers) -> BrowserModifiers {
    BrowserModifiers {
        alt: modifiers.alt,
        ctrl: modifiers.ctrl,
        // `command` is the platform-primary shortcut flag and is also true
        // for physical Ctrl on Linux/Windows. CDP Meta must represent the
        // physical macOS Command key only.
        meta: modifiers.mac_cmd,
        shift: modifiers.shift,
    }
}

fn key_modifiers(ui: &Ui) -> BrowserModifiers {
    ui.input(|i| to_browser_modifiers(i.modifiers))
}

fn clipboard_modifiers(ui: &Ui) -> BrowserModifiers {
    ensure_clipboard_primary(key_modifiers(ui))
}

fn ensure_clipboard_primary(mut modifiers: BrowserModifiers) -> BrowserModifiers {
    if !modifiers.ctrl && !modifiers.meta {
        #[cfg(target_os = "macos")]
        {
            modifiers.meta = true;
        }
        #[cfg(not(target_os = "macos"))]
        {
            modifiers.ctrl = true;
        }
    }
    modifiers
}

fn to_page_coords(rect: egui::Rect, frame_size: [f32; 2], pos: egui::Pos2) -> (f64, f64) {
    let x = f64::from(((pos.x - rect.min.x) / rect.width() * frame_size[0]).clamp(0.0, frame_size[0]));
    let y = f64::from(((pos.y - rect.min.y) / rect.height() * frame_size[1]).clamp(0.0, frame_size[1]));
    (x, y)
}

fn button_mask(button: BrowserButton) -> u32 {
    match button {
        BrowserButton::Left => BUTTON_LEFT,
        BrowserButton::Middle => BUTTON_MIDDLE,
        BrowserButton::Right => BUTTON_RIGHT,
    }
}

fn key_to_browser_key(key: Key) -> Option<BrowserKey> {
    Some(match key {
        Key::ArrowUp => BrowserKey::ArrowUp,
        Key::ArrowDown => BrowserKey::ArrowDown,
        Key::ArrowLeft => BrowserKey::ArrowLeft,
        Key::ArrowRight => BrowserKey::ArrowRight,
        Key::Enter => BrowserKey::Enter,
        Key::Tab => BrowserKey::Tab,
        Key::Escape => BrowserKey::Escape,
        Key::Space => BrowserKey::Space,
        Key::Backspace => BrowserKey::Backspace,
        Key::Delete => BrowserKey::Delete,
        Key::Home => BrowserKey::Home,
        Key::End => BrowserKey::End,
        Key::PageUp => BrowserKey::PageUp,
        Key::PageDown => BrowserKey::PageDown,
        Key::Insert => BrowserKey::Insert,
        Key::F1 => BrowserKey::F1,
        Key::F2 => BrowserKey::F2,
        Key::F3 => BrowserKey::F3,
        Key::F4 => BrowserKey::F4,
        Key::F5 => BrowserKey::F5,
        Key::F6 => BrowserKey::F6,
        Key::F7 => BrowserKey::F7,
        Key::F8 => BrowserKey::F8,
        Key::F9 => BrowserKey::F9,
        Key::F10 => BrowserKey::F10,
        Key::F11 => BrowserKey::F11,
        Key::F12 => BrowserKey::F12,
        Key::A => BrowserKey::Char('a'),
        Key::B => BrowserKey::Char('b'),
        Key::C => BrowserKey::Char('c'),
        Key::D => BrowserKey::Char('d'),
        Key::E => BrowserKey::Char('e'),
        Key::F => BrowserKey::Char('f'),
        Key::G => BrowserKey::Char('g'),
        Key::H => BrowserKey::Char('h'),
        Key::I => BrowserKey::Char('i'),
        Key::J => BrowserKey::Char('j'),
        Key::K => BrowserKey::Char('k'),
        Key::L => BrowserKey::Char('l'),
        Key::M => BrowserKey::Char('m'),
        Key::N => BrowserKey::Char('n'),
        Key::O => BrowserKey::Char('o'),
        Key::P => BrowserKey::Char('p'),
        Key::Q => BrowserKey::Char('q'),
        Key::R => BrowserKey::Char('r'),
        Key::S => BrowserKey::Char('s'),
        Key::T => BrowserKey::Char('t'),
        Key::U => BrowserKey::Char('u'),
        Key::V => BrowserKey::Char('v'),
        Key::W => BrowserKey::Char('w'),
        Key::X => BrowserKey::Char('x'),
        Key::Y => BrowserKey::Char('y'),
        Key::Z => BrowserKey::Char('z'),
        Key::Num0 => BrowserKey::Char('0'),
        Key::Num1 => BrowserKey::Char('1'),
        Key::Num2 => BrowserKey::Char('2'),
        Key::Num3 => BrowserKey::Char('3'),
        Key::Num4 => BrowserKey::Char('4'),
        Key::Num5 => BrowserKey::Char('5'),
        Key::Num6 => BrowserKey::Char('6'),
        Key::Num7 => BrowserKey::Char('7'),
        Key::Num8 => BrowserKey::Char('8'),
        Key::Num9 => BrowserKey::Char('9'),
        Key::Comma => BrowserKey::Char(','),
        Key::Period => BrowserKey::Char('.'),
        Key::Slash => BrowserKey::Char('/'),
        Key::Semicolon => BrowserKey::Char(';'),
        Key::Quote => BrowserKey::Char('\''),
        Key::Minus => BrowserKey::Char('-'),
        Key::Equals => BrowserKey::Char('='),
        Key::OpenBracket => BrowserKey::Char('['),
        Key::CloseBracket => BrowserKey::Char(']'),
        Key::Backslash => BrowserKey::Char('\\'),
        Key::Backtick => BrowserKey::Char('`'),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, shift: bool) -> Event {
        modified_press(
            key,
            Modifiers {
                shift,
                ..Modifiers::NONE
            },
        )
    }

    fn modified_press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn committed_text_is_layout_authoritative() {
        let events = [press(Key::Semicolon, true), Event::Text(";".to_owned())];

        assert_eq!(committed_text_after_key(&events, 0), Some(';'));
    }

    #[test]
    fn committed_text_does_not_cross_the_next_key_event() {
        let events = [press(Key::A, false), press(Key::B, false), Event::Text("b".to_owned())];

        assert_eq!(committed_text_after_key(&events, 0), None);
        assert_eq!(committed_text_after_key(&events, 1), Some('b'));
    }

    #[test]
    fn committed_text_does_not_cross_a_multi_character_text_event() {
        let events = [
            press(Key::A, false),
            Event::Text("paste".to_owned()),
            Event::Text("a".to_owned()),
        ];

        assert_eq!(committed_text_after_key(&events, 0), None);
    }

    #[test]
    fn app_shortcuts_are_filtered_only_when_the_app_owns_them() {
        let shortcuts = AppShortcuts::default();
        let escape = press(Key::Escape, false);
        let fullscreen = press(Key::F11, false);
        let save_editor = modified_press(
            Key::S,
            Modifiers {
                shift: true,
                mac_cmd: true,
                command: true,
                ..Modifiers::NONE
            },
        );

        assert!(app_shortcut_press(&fullscreen, &shortcuts, false));
        assert!(!app_shortcut_press(&escape, &shortcuts, false));
        assert!(app_shortcut_press(&escape, &shortcuts, true));
        assert!(!app_shortcut_press(&save_editor, &shortcuts, false));
    }

    #[test]
    fn suppressed_release_is_consumed_after_modifiers_change() {
        let mut state = BrowserUiState::default();
        state.suppressed_shortcut_keys.insert(BrowserKey::Char('r'));

        assert!(consume_suppressed_key(&mut state, Some(BrowserKey::Char('r')), false));
        assert!(!state.suppressed_shortcut_keys.contains(&BrowserKey::Char('r')));
        assert!(!consume_suppressed_key(&mut state, Some(BrowserKey::Char('r')), false));
    }

    #[test]
    fn url_focus_keeps_browser_reload_shortcuts_routable() {
        let mut state = BrowserUiState::default();
        let command_r = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };

        assert!(should_route_key_while_url_focused(&state, Key::F5, Modifiers::NONE));
        assert!(should_route_key_while_url_focused(&state, Key::R, command_r));
        assert!(!should_route_key_while_url_focused(&state, Key::A, Modifiers::NONE));

        state.suppressed_shortcut_keys.insert(BrowserKey::Char('r'));
        assert!(should_route_key_while_url_focused(&state, Key::R, Modifiers::NONE));
    }

    #[test]
    fn covered_panel_ignores_pointer_until_it_owns_capture() {
        assert!(!should_track_pointer(false, true, false));
        assert!(should_track_pointer(true, true, false));
        assert!(should_track_pointer(false, false, true));
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

    #[test]
    fn clipboard_shortcuts_suppress_their_later_native_release() {
        let mut state = BrowserUiState::default();
        state.clipboard_release_keys.insert(BrowserKey::Char('c'));
        state.clipboard_release_keys.insert(BrowserKey::Char('x'));

        assert!(consume_clipboard_release(
            &mut state,
            Some(BrowserKey::Char('c')),
            false
        ));
        assert!(consume_clipboard_release(
            &mut state,
            Some(BrowserKey::Char('x')),
            false
        ));
        assert!(state.clipboard_release_keys.is_empty());
    }

    #[test]
    fn a_new_clipboard_key_press_retires_stale_release_suppression() {
        let mut state = BrowserUiState::default();
        state.clipboard_release_keys.insert(BrowserKey::Char('c'));

        assert!(!consume_clipboard_release(
            &mut state,
            Some(BrowserKey::Char('c')),
            true
        ));
        assert!(state.clipboard_release_keys.is_empty());
    }

    #[test]
    fn clipboard_pseudo_events_restore_a_released_primary_modifier() {
        let modifiers = ensure_clipboard_primary(BrowserModifiers::default());

        #[cfg(target_os = "macos")]
        assert!(modifiers.meta);
        #[cfg(not(target_os = "macos"))]
        assert!(modifiers.ctrl);
    }

    #[test]
    fn primary_a_requests_chromium_select_all() {
        let modifiers = if cfg!(target_os = "macos") {
            BrowserModifiers {
                meta: true,
                ..BrowserModifiers::none()
            }
        } else {
            BrowserModifiers {
                ctrl: true,
                ..BrowserModifiers::none()
            }
        };

        assert_eq!(editing_command(Key::A, modifiers), Some(BrowserEditCommand::SelectAll));
        assert_eq!(
            editing_command(
                Key::A,
                BrowserModifiers {
                    shift: true,
                    ..modifiers
                }
            ),
            None
        );
    }
}
