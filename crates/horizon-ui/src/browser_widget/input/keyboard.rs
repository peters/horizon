//! Keyboard, IME, clipboard, and browser-shortcut routing.

use egui::{Event, Key, Modifiers, Ui};
use horizon_core::AppShortcuts;
use horizon_core::browser::{
    BrowserCommand, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers, BrowserPanelState,
};

use crate::browser_widget::BrowserUiState;

pub(super) fn events(
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

pub(super) fn browser_shortcut_events(events: &[Event], browser: &BrowserPanelState, state: &mut BrowserUiState) {
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

pub(super) fn release_pressed_keys(
    browser: &BrowserPanelState,
    state: &mut BrowserUiState,
    modifiers: BrowserModifiers,
) {
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

pub(super) fn key_modifiers(ui: &Ui) -> BrowserModifiers {
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
