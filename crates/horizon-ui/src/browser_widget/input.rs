//! egui → CDP input translation for the focused browser panel.

use egui::{Event, Key, Modifiers, PointerButton, Ui};
use horizon_core::AppShortcuts;
use horizon_core::browser::{
    BrowserButton, BrowserCommand, BrowserInput, BrowserKey, BrowserModifiers, BrowserPanelState,
};

use crate::browser_widget::BrowserUiState;

/// CDP `buttons` bitmask for currently-down mouse buttons.
const BUTTON_LEFT: u32 = 1;
const BUTTON_MIDDLE: u32 = 2;
const BUTTON_RIGHT: u32 = 4;

/// Focus/interaction flags for one frame.
#[derive(Clone, Copy)]
pub(crate) struct InputFlags<'a> {
    pub(crate) is_focused: bool,
    pub(crate) interactive: bool,
    pub(crate) url_focused: bool,
    pub(crate) shortcuts: &'a AppShortcuts,
    pub(crate) exit_fullscreen_shortcut_owner: ShortcutOwner,
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
    flags: InputFlags<'_>,
) {
    if !flags.interactive {
        return;
    }
    if let (Some(rect), Some(frame_size)) = (body, frame_size) {
        pointer_events(ui, browser, state, rect, frame_size);
    }
    if flags.is_focused && !flags.url_focused {
        keyboard_events(
            ui,
            browser,
            state,
            flags.shortcuts,
            matches!(flags.exit_fullscreen_shortcut_owner, ShortcutOwner::App),
        );
    }
}

fn pointer_events(
    ui: &Ui,
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    rect: egui::Rect,
    frame_size: [f32; 2],
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
    // Frame-end pointer position: only used for wheel events, which carry
    // no per-event position.
    let wheel_pos = frame_pos.map(transform).filter(|p| rect.contains(*p));

    // Replay this frame's pointer events with their own positions. A fast
    // gesture (press, moves, release) can be batched into a single
    // rendered frame; sampling only the frame-end position would collapse
    // the whole drag to one point. A panel only owns a press that started
    // inside its body, so a drag released over another panel (or the
    // canvas) still ends with exactly one release. Chrome coalesces adjacent
    // mouseMoved messages, so only the final move in each adjacent run is
    // forwarded while button and wheel ordering is preserved.
    let mut event_buttons = state.captured_button.map_or(0, button_mask);
    let mut pending_move: Option<(f64, f64, u32)> = None;
    for event in ctx.input(|i| i.events.clone()) {
        match event {
            Event::PointerButton {
                pos, button, pressed, ..
            } => {
                flush_pending_move(browser, &mut pending_move);
                let Some(browser_button) = egui_button(button) else {
                    continue;
                };
                let p = transform(pos);
                let (x, y) = to_page_coords(rect, frame_size, p);
                if pressed {
                    if rect.contains(p) {
                        state.captured_button = Some(browser_button);
                        state.last_mouse = Some(p);
                        event_buttons |= button_mask(browser_button);
                        browser.send(BrowserCommand::Input(BrowserInput::MousePress {
                            x,
                            y,
                            button: browser_button,
                            click_count: 1,
                            buttons: event_buttons,
                            modifiers,
                        }));
                    }
                } else if state.captured_button == Some(browser_button) {
                    state.captured_button = None;
                    state.last_mouse = Some(p);
                    event_buttons &= !button_mask(browser_button);
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button: browser_button,
                        click_count: 1,
                        buttons: event_buttons,
                        modifiers,
                    }));
                }
            }
            Event::PointerMoved(pos) => {
                let p = transform(pos);
                let tracking = rect.contains(p) || state.captured_button.is_some();
                // Movement dedup: only forward real movement. `None` (the
                // first move, or the first after PointerGone) must be
                // forwarded, or hover would be dead until a click.
                if tracking && state.last_mouse.is_none_or(|last| (last - p).length() >= 0.5) {
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    state.last_mouse = Some(p);
                    pending_move = Some((x, y, event_buttons));
                }
            }
            // The pointer left the window mid-drag: end the drag where we
            // last saw it instead of stranding Chrome's button state down.
            Event::PointerGone => {
                flush_pending_move(browser, &mut pending_move);
                if let Some(button) = state.captured_button.take()
                    && let Some(p) = state.last_mouse
                {
                    let (x, y) = to_page_coords(rect, frame_size, p);
                    event_buttons &= !button_mask(button);
                    browser.send(BrowserCommand::Input(BrowserInput::MouseRelease {
                        x,
                        y,
                        button,
                        click_count: 1,
                        buttons: event_buttons,
                        modifiers,
                    }));
                }
                state.last_mouse = None;
            }
            Event::MouseWheel { unit, delta, .. } => {
                flush_pending_move(browser, &mut pending_move);
                if let Some(p) = wheel_pos {
                    let scale = match unit {
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

fn flush_pending_move(browser: &BrowserPanelState, pending_move: &mut Option<(f64, f64, u32)>) {
    if let Some((x, y, buttons)) = pending_move.take() {
        browser.send(BrowserCommand::Input(BrowserInput::MouseMove { x, y, buttons }));
    }
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
    browser: &mut BrowserPanelState,
    state: &mut BrowserUiState,
    shortcuts: &AppShortcuts,
    exit_fullscreen_shortcut_active: bool,
) {
    let KeyboardFrameEvents {
        events,
        key_texts,
        mut key_chars,
        mut shortcut_chars,
        shortcut_presses,
    } = collect_keyboard_frame_events(ui, shortcuts, exit_fullscreen_shortcut_active);
    for (event_index, event) in events.into_iter().enumerate() {
        match event {
            Event::Text(text) if !text.is_empty() => {
                let shortcut_text = take_matching_single_char(&text, &mut shortcut_chars);
                let duplicate_key_text = take_matching_single_char(&text, &mut key_chars);
                if !shortcut_text && !duplicate_key_text {
                    browser.send(BrowserCommand::Input(BrowserInput::InsertText { text }));
                }
            }
            // egui_winit turns Ctrl/Cmd+V into a global Paste event (the URL
            // bar consumes its own copy while focused). Both paste and IME
            // commits reach the page as one CDP insertText operation.
            Event::Ime(egui::ImeEvent::Commit(text)) | Event::Paste(text) if !text.is_empty() => {
                browser.send(BrowserCommand::Input(BrowserInput::InsertText { text }));
            }
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
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    key_text: key_texts[event_index],
                    app_shortcut: shortcut_presses[event_index],
                },
            ),
            _ => {}
        }
    }
}

struct KeyboardFrameEvents {
    events: Vec<Event>,
    key_texts: Vec<Option<char>>,
    key_chars: Vec<char>,
    shortcut_chars: Vec<char>,
    shortcut_presses: Vec<bool>,
}

fn collect_keyboard_frame_events(
    ui: &Ui,
    shortcuts: &AppShortcuts,
    exit_fullscreen_shortcut_active: bool,
) -> KeyboardFrameEvents {
    let events = ui.ctx().input(|i| i.events.clone());
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
                committed_text_after_key(&events, index)
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
        .filter_map(|(index, _)| committed_text_after_key(&events, index))
        .collect();
    KeyboardFrameEvents {
        events,
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
    let reload_shortcut = key == Key::F5 || (key == Key::R && (modifiers.ctrl || modifiers.mac_cmd));
    let browser_key = key_to_browser_key(key);
    if app_shortcut {
        if let Some(browser_key) = browser_key {
            state.suppressed_shortcut_keys.insert(browser_key);
            state.pressed_key_text.remove(&browser_key);
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
                state.pressed_key_text.remove(&browser_key);
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
            browser.send(BrowserCommand::Input(BrowserInput::KeyUp {
                key: browser_key,
                text: state.pressed_key_text.remove(&browser_key),
                modifiers: to_browser_modifiers(modifiers),
            }));
        }
        return;
    }
    let Some(browser_key) = browser_key else {
        return;
    };
    let modifiers = to_browser_modifiers(modifiers);
    let text = key_text
        .map(|character| character.to_string())
        .filter(|_| !(modifiers.ctrl || modifiers.meta || modifiers.alt));
    if let Some(text) = &text {
        state.pressed_key_text.insert(browser_key, text.clone());
    } else {
        state.pressed_key_text.remove(&browser_key);
    }
    browser.send(BrowserCommand::Input(BrowserInput::KeyDown {
        key: browser_key,
        text,
        modifiers,
        repeat,
    }));
}

fn consume_suppressed_key(state: &mut BrowserUiState, key: Option<BrowserKey>, pressed: bool) -> bool {
    let Some(key) = key.filter(|key| state.suppressed_shortcut_keys.contains(key)) else {
        return false;
    };
    if !pressed {
        state.suppressed_shortcut_keys.remove(&key);
        state.pressed_key_text.remove(&key);
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
}
