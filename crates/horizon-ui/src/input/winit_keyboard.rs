use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use egui::{Event, Key, Modifiers, RawInput};
use winit::event::KeyEvent;
use winit::keyboard::{Key as WinitKey, KeyCode, NamedKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalInputEvent {
    pub(crate) event: Event,
    pub(crate) key_without_modifiers_text: Option<String>,
    pub(crate) observed_key: Option<FrameKeyEvent>,
}

impl TerminalInputEvent {
    fn new(event: Event, observed_key: Option<FrameKeyEvent>) -> Self {
        let key_without_modifiers_text = observed_key
            .as_ref()
            .and_then(|frame_key| frame_key.key_without_modifiers_text.clone());
        Self {
            event,
            key_without_modifiers_text,
            observed_key,
        }
    }

    pub(crate) fn is_plain_ctrl_c_copy_command(&self) -> bool {
        self.observed_key
            .as_ref()
            .is_some_and(FrameKeyEvent::is_plain_ctrl_c_copy_command)
    }
}

#[derive(Clone, Default)]
pub(crate) struct ObservedKeyboardInputs(Arc<Mutex<VecDeque<ObservedKeyboardEvent>>>);

impl ObservedKeyboardInputs {
    pub(crate) fn observe(&self, event: &KeyEvent, modifiers: Modifiers) {
        let Some(observed) = ObservedKeyboardEvent::from_winit(event, modifiers) else {
            return;
        };

        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(observed);
    }

    pub(crate) fn take_frame_key_events(&self, raw_input: &RawInput) -> Vec<FrameKeyEvent> {
        if !raw_input.events.iter().any(EventClassifier::is_keyboard_output) {
            return Vec::new();
        }

        let mut observed = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        align_observed_keyboard_events(&raw_input.events, std::mem::take(&mut *observed))
    }
}

pub(crate) fn terminal_input_events(events: &[Event], frame_key_events: Vec<FrameKeyEvent>) -> Vec<TerminalInputEvent> {
    let mut frame_key_events: VecDeque<_> = frame_key_events.into();
    let mut terminal_events = Vec::with_capacity(events.len());

    for event in events {
        let observed_key = match event {
            Event::Key {
                key,
                physical_key,
                pressed,
                modifiers,
                ..
            } => consume_matching(&mut frame_key_events, |candidate| {
                candidate.matches(*key, *physical_key, *pressed, *modifiers)
            }),
            Event::Cut => consume_matching(&mut frame_key_events, FrameKeyEvent::is_cut),
            Event::Copy => consume_matching(&mut frame_key_events, FrameKeyEvent::is_copy),
            Event::Paste(_) => consume_matching(&mut frame_key_events, FrameKeyEvent::is_paste),
            _ => None,
        };

        terminal_events.push(TerminalInputEvent::new(event.clone(), observed_key));
    }

    terminal_events
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FrameKeyEvent {
    kind: KeyboardOutputKind,
    key: Key,
    physical_key: Option<Key>,
    pressed: bool,
    modifiers: Modifiers,
    pub(crate) key_without_modifiers_text: Option<String>,
}

impl FrameKeyEvent {
    fn matches(&self, key: Key, physical_key: Option<Key>, pressed: bool, modifiers: Modifiers) -> bool {
        self.kind == KeyboardOutputKind::Key
            && self.key == key
            && self.physical_key == physical_key
            && self.pressed == pressed
            && self.modifiers == modifiers
    }

    fn is_cut(&self) -> bool {
        self.kind == KeyboardOutputKind::Cut
    }

    fn is_copy(&self) -> bool {
        self.kind == KeyboardOutputKind::Copy
    }

    fn is_paste(&self) -> bool {
        self.kind == KeyboardOutputKind::Paste
    }

    fn is_plain_ctrl_c_copy_command(&self) -> bool {
        self.kind == KeyboardOutputKind::Copy
            && self.key == Key::C
            && self.modifiers.ctrl
            && !self.modifiers.mac_cmd
            && !self.modifiers.shift
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyboardOutputKind {
    Key,
    Cut,
    Copy,
    Paste,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedKeyboardEvent {
    kind: KeyboardOutputKind,
    key: Key,
    physical_key: Option<Key>,
    pressed: bool,
    modifiers: Modifiers,
    key_without_modifiers_text: Option<String>,
}

impl ObservedKeyboardEvent {
    fn from_winit(event: &KeyEvent, modifiers: Modifiers) -> Option<Self> {
        let physical_key = physical_key_from_winit(event.physical_key);
        let logical_key = key_from_winit_key(&event.logical_key);
        let key = logical_key.or(physical_key)?;
        let pressed = event.state.is_pressed();
        let kind = if pressed && is_cut_command(modifiers, key) {
            KeyboardOutputKind::Cut
        } else if pressed && is_copy_command(modifiers, key) {
            KeyboardOutputKind::Copy
        } else if pressed && is_paste_command(modifiers, key) {
            KeyboardOutputKind::Paste
        } else {
            KeyboardOutputKind::Key
        };

        Some(Self {
            kind,
            key,
            physical_key,
            pressed,
            modifiers,
            key_without_modifiers_text: event
                .key_without_modifiers()
                .to_text()
                .filter(|text| !text.is_empty())
                .map(ToOwned::to_owned),
        })
    }

    fn matches_key_event(&self, key: Key, physical_key: Option<Key>, pressed: bool, modifiers: Modifiers) -> bool {
        self.kind == KeyboardOutputKind::Key
            && self.key == key
            && self.physical_key == physical_key
            && self.pressed == pressed
            && self.modifiers == modifiers
    }
}

struct EventClassifier;

impl EventClassifier {
    fn is_keyboard_output(event: &Event) -> bool {
        matches!(event, Event::Key { .. } | Event::Cut | Event::Copy | Event::Paste(_))
    }
}

fn align_observed_keyboard_events(events: &[Event], observed: VecDeque<ObservedKeyboardEvent>) -> Vec<FrameKeyEvent> {
    let mut observed = observed;
    let mut frame_key_events = Vec::new();

    for event in events {
        match event {
            Event::Key {
                key,
                physical_key,
                pressed,
                modifiers,
                ..
            } => {
                if let Some(observed_event) = consume_matching(&mut observed, |candidate| {
                    candidate.matches_key_event(*key, *physical_key, *pressed, *modifiers)
                }) {
                    frame_key_events.push(FrameKeyEvent {
                        kind: observed_event.kind,
                        key: observed_event.key,
                        physical_key: observed_event.physical_key,
                        pressed: observed_event.pressed,
                        modifiers: observed_event.modifiers,
                        key_without_modifiers_text: observed_event.key_without_modifiers_text,
                    });
                }
            }
            Event::Cut => {
                if let Some(observed_event) =
                    consume_matching(&mut observed, |candidate| candidate.kind == KeyboardOutputKind::Cut)
                {
                    frame_key_events.push(FrameKeyEvent {
                        kind: observed_event.kind,
                        key: observed_event.key,
                        physical_key: observed_event.physical_key,
                        pressed: observed_event.pressed,
                        modifiers: observed_event.modifiers,
                        key_without_modifiers_text: observed_event.key_without_modifiers_text,
                    });
                }
            }
            Event::Copy => {
                if let Some(observed_event) =
                    consume_matching(&mut observed, |candidate| candidate.kind == KeyboardOutputKind::Copy)
                {
                    frame_key_events.push(FrameKeyEvent {
                        kind: observed_event.kind,
                        key: observed_event.key,
                        physical_key: observed_event.physical_key,
                        pressed: observed_event.pressed,
                        modifiers: observed_event.modifiers,
                        key_without_modifiers_text: observed_event.key_without_modifiers_text,
                    });
                }
            }
            Event::Paste(_) => {
                if let Some(observed_event) =
                    consume_matching(&mut observed, |candidate| candidate.kind == KeyboardOutputKind::Paste)
                {
                    frame_key_events.push(FrameKeyEvent {
                        kind: observed_event.kind,
                        key: observed_event.key,
                        physical_key: observed_event.physical_key,
                        pressed: observed_event.pressed,
                        modifiers: observed_event.modifiers,
                        key_without_modifiers_text: observed_event.key_without_modifiers_text,
                    });
                }
            }
            _ => {}
        }
    }

    frame_key_events
}

fn consume_matching<T>(queue: &mut VecDeque<T>, mut predicate: impl FnMut(&T) -> bool) -> Option<T> {
    while let Some(candidate) = queue.pop_front() {
        if predicate(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn is_cut_command(modifiers: Modifiers, keycode: Key) -> bool {
    keycode == Key::Cut
        || (modifiers.command && keycode == Key::X)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == Key::Delete)
}

fn is_copy_command(modifiers: Modifiers, keycode: Key) -> bool {
    keycode == Key::Copy
        || (modifiers.command && keycode == Key::C)
        || (cfg!(target_os = "windows") && modifiers.ctrl && keycode == Key::Insert)
}

fn is_paste_command(modifiers: Modifiers, keycode: Key) -> bool {
    keycode == Key::Paste
        || (modifiers.command && keycode == Key::V)
        || (cfg!(target_os = "windows") && modifiers.shift && keycode == Key::Insert)
}

fn key_from_winit_key(key: &WinitKey) -> Option<Key> {
    match key {
        WinitKey::Named(named_key) => key_from_named_key(*named_key),
        WinitKey::Character(text) => Key::from_name(text.as_str()),
        WinitKey::Unidentified(_) | WinitKey::Dead(_) => None,
    }
}

fn physical_key_from_winit(key: PhysicalKey) -> Option<Key> {
    match key {
        PhysicalKey::Code(keycode) => key_from_key_code(keycode),
        PhysicalKey::Unidentified(_) => None,
    }
}

fn key_from_named_key(named_key: NamedKey) -> Option<Key> {
    Some(match named_key {
        NamedKey::Enter => Key::Enter,
        NamedKey::Tab => Key::Tab,
        NamedKey::ArrowDown => Key::ArrowDown,
        NamedKey::ArrowLeft => Key::ArrowLeft,
        NamedKey::ArrowRight => Key::ArrowRight,
        NamedKey::ArrowUp => Key::ArrowUp,
        NamedKey::End => Key::End,
        NamedKey::Home => Key::Home,
        NamedKey::PageDown => Key::PageDown,
        NamedKey::PageUp => Key::PageUp,
        NamedKey::Backspace => Key::Backspace,
        NamedKey::Delete => Key::Delete,
        NamedKey::Insert => Key::Insert,
        NamedKey::Escape => Key::Escape,
        NamedKey::Cut => Key::Cut,
        NamedKey::Copy => Key::Copy,
        NamedKey::Paste => Key::Paste,
        NamedKey::Space => Key::Space,
        NamedKey::F1 => Key::F1,
        NamedKey::F2 => Key::F2,
        NamedKey::F3 => Key::F3,
        NamedKey::F4 => Key::F4,
        NamedKey::F5 => Key::F5,
        NamedKey::F6 => Key::F6,
        NamedKey::F7 => Key::F7,
        NamedKey::F8 => Key::F8,
        NamedKey::F9 => Key::F9,
        NamedKey::F10 => Key::F10,
        NamedKey::F11 => Key::F11,
        NamedKey::F12 => Key::F12,
        NamedKey::F13 => Key::F13,
        NamedKey::F14 => Key::F14,
        NamedKey::F15 => Key::F15,
        NamedKey::F16 => Key::F16,
        NamedKey::F17 => Key::F17,
        NamedKey::F18 => Key::F18,
        NamedKey::F19 => Key::F19,
        NamedKey::F20 => Key::F20,
        NamedKey::F21 => Key::F21,
        NamedKey::F22 => Key::F22,
        NamedKey::F23 => Key::F23,
        NamedKey::F24 => Key::F24,
        NamedKey::F25 => Key::F25,
        NamedKey::F26 => Key::F26,
        NamedKey::F27 => Key::F27,
        NamedKey::F28 => Key::F28,
        NamedKey::F29 => Key::F29,
        NamedKey::F30 => Key::F30,
        NamedKey::F31 => Key::F31,
        NamedKey::F32 => Key::F32,
        NamedKey::F33 => Key::F33,
        NamedKey::F34 => Key::F34,
        NamedKey::F35 => Key::F35,
        NamedKey::BrowserBack => Key::BrowserBack,
        _ => return None,
    })
}

fn key_from_key_code(key: KeyCode) -> Option<Key> {
    Some(match key {
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Space => Key::Space,
        KeyCode::Comma => Key::Comma,
        KeyCode::Period => Key::Period,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Backslash => Key::Backslash,
        KeyCode::Slash | KeyCode::NumpadDivide => Key::Slash,
        KeyCode::BracketLeft => Key::OpenBracket,
        KeyCode::BracketRight => Key::CloseBracket,
        KeyCode::Backquote => Key::Backtick,
        KeyCode::Quote => Key::Quote,
        KeyCode::Cut => Key::Cut,
        KeyCode::Copy => Key::Copy,
        KeyCode::Paste => Key::Paste,
        KeyCode::Minus | KeyCode::NumpadSubtract => Key::Minus,
        KeyCode::NumpadAdd => Key::Plus,
        KeyCode::Equal => Key::Equals,
        KeyCode::Digit0 | KeyCode::Numpad0 => Key::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => Key::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => Key::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => Key::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => Key::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => Key::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => Key::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => Key::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => Key::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => Key::Num9,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FrameKeyEvent, KeyboardOutputKind, ObservedKeyboardEvent, align_observed_keyboard_events, terminal_input_events,
    };
    use egui::{Event, Key, Modifiers, RawInput};

    #[test]
    fn copy_command_keeps_its_origin_and_following_key_context() {
        let events = vec![
            Event::Copy,
            Event::Key {
                key: Key::OpenBracket,
                physical_key: Some(Key::OpenBracket),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            },
        ];

        let observed = [
            ObservedKeyboardEvent {
                kind: KeyboardOutputKind::Copy,
                key: Key::C,
                physical_key: Some(Key::C),
                pressed: true,
                modifiers: Modifiers::CTRL,
                key_without_modifiers_text: Some("c".to_owned()),
            },
            ObservedKeyboardEvent {
                kind: KeyboardOutputKind::Key,
                key: Key::OpenBracket,
                physical_key: Some(Key::OpenBracket),
                pressed: true,
                modifiers: Modifiers::SHIFT,
                key_without_modifiers_text: Some("å".to_owned()),
            },
        ]
        .into();

        let frame = align_observed_keyboard_events(&events, observed);

        assert_eq!(
            frame,
            vec![
                FrameKeyEvent {
                    kind: KeyboardOutputKind::Copy,
                    key: Key::C,
                    physical_key: Some(Key::C),
                    pressed: true,
                    modifiers: Modifiers::CTRL,
                    key_without_modifiers_text: Some("c".to_owned()),
                },
                FrameKeyEvent {
                    kind: KeyboardOutputKind::Key,
                    key: Key::OpenBracket,
                    physical_key: Some(Key::OpenBracket),
                    pressed: true,
                    modifiers: Modifiers::SHIFT,
                    key_without_modifiers_text: Some("å".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn terminal_input_events_attach_key_without_modifiers_text() {
        let events = vec![
            Event::Key {
                key: Key::OpenBracket,
                physical_key: Some(Key::OpenBracket),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::SHIFT,
            },
            Event::Text("Å".to_owned()),
        ];

        let terminal_events = terminal_input_events(
            &events,
            vec![FrameKeyEvent {
                kind: KeyboardOutputKind::Key,
                key: Key::OpenBracket,
                physical_key: Some(Key::OpenBracket),
                pressed: true,
                modifiers: Modifiers::SHIFT,
                key_without_modifiers_text: Some("å".to_owned()),
            }],
        );

        assert_eq!(terminal_events[0].key_without_modifiers_text.as_deref(), Some("å"));
        assert_eq!(terminal_events[1].key_without_modifiers_text, None);
        assert_eq!(
            terminal_events[0].observed_key,
            Some(FrameKeyEvent {
                kind: KeyboardOutputKind::Key,
                key: Key::OpenBracket,
                physical_key: Some(Key::OpenBracket),
                pressed: true,
                modifiers: Modifiers::SHIFT,
                key_without_modifiers_text: Some("å".to_owned()),
            })
        );
    }

    #[test]
    fn terminal_input_events_keep_copy_shortcut_origin() {
        let terminal_events = terminal_input_events(
            &[Event::Copy],
            vec![FrameKeyEvent {
                kind: KeyboardOutputKind::Copy,
                key: Key::C,
                physical_key: Some(Key::C),
                pressed: true,
                modifiers: Modifiers::CTRL,
                key_without_modifiers_text: Some("c".to_owned()),
            }],
        );

        assert!(terminal_events[0].is_plain_ctrl_c_copy_command());
    }

    #[test]
    fn empty_keyboard_frames_do_not_consume_observed_state() {
        let observed = super::ObservedKeyboardInputs::default();
        observed
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(ObservedKeyboardEvent {
                kind: KeyboardOutputKind::Key,
                key: Key::A,
                physical_key: Some(Key::A),
                pressed: true,
                modifiers: Modifiers::NONE,
                key_without_modifiers_text: Some("a".to_owned()),
            });

        let frame = observed.take_frame_key_events(&RawInput::default());
        assert!(frame.is_empty());

        let remaining = observed
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        assert_eq!(remaining, 1);
    }
}
