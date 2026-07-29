use std::collections::VecDeque;

use alacritty_terminal::term::TermMode;
use egui::Key;
use horizon_core::{Panel, PanelKind, ShortcutBinding, ShortcutKey, ShortcutModifiers, SshConnectionStatus};

use super::super::ime::{prepare_terminal_keyboard_events, store_terminal_ime_enabled, terminal_ime_enabled};
use crate::app::shortcuts::shortcut_event_matches;
use crate::input::{self, TerminalInputEvent};
use crate::primary_selection::PrimarySelection;

pub(crate) const SSH_RECONNECT_SHORTCUT: ShortcutBinding =
    ShortcutBinding::new(ShortcutModifiers::PRIMARY_SHIFT, ShortcutKey::Letter('R'));

pub(crate) fn handle_terminal_keyboard_input(
    ui: &egui::Ui,
    terminal_id: egui::Id,
    panel: &mut Panel,
    events: &[TerminalInputEvent],
    primary_selection: &PrimarySelection,
    local_ssh_reconnect_enabled: bool,
) -> bool {
    if local_ssh_reconnect_enabled && disconnected_ssh_reconnect_requested(panel.kind, panel.ssh_status(), events) {
        return true;
    }

    let Some(terminal) = panel.terminal_mut() else {
        return false;
    };
    let mode = terminal.mode();
    let mut forwarder = KeyboardInputForwarder::default();
    let mut ime_enabled = terminal_ime_enabled(ui, terminal_id);
    let events = prepare_terminal_keyboard_events(events, ime_enabled);

    for event in &events {
        match &event.event {
            egui::Event::Ime(egui::ImeEvent::Enabled | egui::ImeEvent::Preedit(_)) => {
                ime_enabled = true;
            }
            egui::Event::Ime(egui::ImeEvent::Disabled) => {
                ime_enabled = false;
            }
            egui::Event::Text(text) | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if matches!(&event.event, egui::Event::Ime(egui::ImeEvent::Commit(_))) {
                    ime_enabled = false;
                }
                let emission = forwarder.on_text(text, mode);
                if emission.clears_selection {
                    terminal.clear_selection();
                }
                if !emission.bytes.is_empty() {
                    terminal.write_input(&emission.bytes);
                }
            }
            egui::Event::Paste(text) => {
                terminal.clear_selection();
                let bytes = input::paste_bytes(text, mode, true);
                terminal.write_input(&bytes);
            }
            egui::Event::Copy => {
                if event.is_plain_ctrl_c_copy_command() {
                    terminal.write_input(&[3]);
                } else if let Some(text) = terminal.selection_to_string() {
                    primary_selection.copy(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
            }
            egui::Event::Cut => {
                if let Some(text) = terminal.selection_to_string() {
                    primary_selection.copy(&text);
                    ui.ctx().copy_text(text);
                    terminal.clear_selection();
                }
                terminal.write_input(&[24]);
            }
            egui::Event::Key { .. } => {
                let emission = forwarder.on_key(event, mode);
                if !emission.bytes.is_empty() {
                    terminal.write_input(&emission.bytes);
                }
            }
            _ => {}
        }
    }

    let emission = forwarder.finish();
    if !emission.bytes.is_empty() {
        terminal.write_input(&emission.bytes);
    }

    store_terminal_ime_enabled(ui, terminal_id, ime_enabled);

    false
}

fn disconnected_ssh_reconnect_requested(
    kind: PanelKind,
    ssh_status: Option<SshConnectionStatus>,
    events: &[TerminalInputEvent],
) -> bool {
    kind == PanelKind::Ssh
        && matches!(ssh_status, Some(SshConnectionStatus::Disconnected))
        && events.iter().any(|input_event| {
            matches!(
                &input_event.event,
                egui::Event::Key {
                    pressed: true,
                    repeat: false,
                    ..
                }
            ) && shortcut_event_matches(&input_event.event, SSH_RECONNECT_SHORTCUT)
        })
}

#[derive(Default)]
struct KeyboardInputForwarder {
    suppressed_text: VecDeque<String>,
    deferred_text_key: Option<DeferredTextKey>,
}

impl KeyboardInputForwarder {
    fn on_text(&mut self, text: &str, mode: TermMode) -> InputEmission {
        if let Some(mut deferred) = self.deferred_text_key.take() {
            if let Some(actual_text) = deferred.synthetic_text.as_deref() {
                if actual_text != text {
                    // Drop stale synthetic state if a later text event does not
                    // belong to the deferred key.
                    return InputEmission::raw_text(text);
                }
            } else {
                let emission = deferred.resolve_text(text, mode);
                if deferred.synthetic_text.is_some() {
                    self.deferred_text_key = Some(deferred);
                }
                return emission;
            }
        }

        if self.suppressed_text.front().is_some_and(|expected| expected == text) {
            self.suppressed_text.pop_front();
            return InputEmission::default();
        }

        InputEmission::raw_text(text)
    }

    fn on_key(&mut self, input_event: &TerminalInputEvent, mode: TermMode) -> InputEmission {
        let egui::Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            modifiers,
            ..
        } = &input_event.event
        else {
            return InputEmission::default();
        };

        let key_identity =
            input::KeyIdentity::new(*key, *physical_key, input_event.key_without_modifiers_text.as_deref());
        let context = input::KeyEventContext::new(*pressed, *repeat, *modifiers, mode);
        let mut emission = InputEmission::default();

        if let Some(deferred) = self.deferred_text_key.as_mut() {
            if let Some(actual_text) = deferred.synthetic_text.as_deref() {
                if !pressed && deferred.matches(*key, *physical_key) {
                    if let Some(translation) = input::translate_text_event(
                        input::KeyIdentity::new(*key, *physical_key, deferred.key_without_modifiers_text.as_deref()),
                        actual_text,
                        input::KeyEventContext::new(false, *repeat, *modifiers, mode),
                    ) {
                        emission.bytes.extend_from_slice(&translation.bytes);
                    }
                    self.deferred_text_key = None;
                    return emission;
                }

                if !deferred.matches(*key, *physical_key) {
                    self.deferred_text_key = None;
                }
            } else if !pressed && deferred.matches(*key, *physical_key) {
                deferred.release_seen = true;
                deferred.release_translation = input::translate_key_event_with_physical(
                    key_identity,
                    input::KeyEventContext::new(false, *repeat, *modifiers, mode),
                );
                return emission;
            } else if !deferred.matches(*key, *physical_key) {
                emission.bytes.extend_from_slice(&deferred.flush_fallback());
                self.deferred_text_key = None;
            }
        }

        if let Some(translation) = input::translate_key_event_with_physical(key_identity, context) {
            if *pressed
                && translation.suppress_text.is_some()
                && (modifiers.alt
                    || mode.intersects(TermMode::KITTY_KEYBOARD_PROTOCOL)
                    || input::should_defer_textual_key(*key, *physical_key, *pressed, *modifiers, mode))
            {
                self.deferred_text_key = Some(DeferredTextKey::new(
                    *key,
                    *physical_key,
                    input_event.key_without_modifiers_text.as_deref(),
                    *modifiers,
                    Some(translation),
                ));
                return emission;
            }

            if let Some(text) = translation.suppress_text {
                self.suppressed_text.push_back(text);
            }
            emission.bytes.extend_from_slice(&translation.bytes);
            return emission;
        }

        if input::should_defer_textual_key(*key, *physical_key, *pressed, *modifiers, mode) {
            self.deferred_text_key = Some(DeferredTextKey::new(
                *key,
                *physical_key,
                input_event.key_without_modifiers_text.as_deref(),
                *modifiers,
                None,
            ));
        }

        emission
    }

    fn finish(&mut self) -> InputEmission {
        let Some(deferred) = self.deferred_text_key.take() else {
            return InputEmission::default();
        };

        if deferred.synthetic_text.is_some() {
            return InputEmission::default();
        }

        InputEmission::pty(deferred.flush_fallback())
    }
}

struct DeferredTextKey {
    key: Key,
    physical_key: Option<Key>,
    key_without_modifiers_text: Option<String>,
    modifiers: egui::Modifiers,
    press_translation: Option<input::KeyTranslation>,
    release_translation: Option<input::KeyTranslation>,
    release_seen: bool,
    synthetic_text: Option<String>,
}

impl DeferredTextKey {
    fn new(
        key: Key,
        physical_key: Option<Key>,
        key_without_modifiers_text: Option<&str>,
        modifiers: egui::Modifiers,
        press_translation: Option<input::KeyTranslation>,
    ) -> Self {
        Self {
            key,
            physical_key,
            key_without_modifiers_text: key_without_modifiers_text.map(ToOwned::to_owned),
            modifiers,
            press_translation,
            release_translation: None,
            release_seen: false,
            synthetic_text: None,
        }
    }

    fn matches(&self, key: Key, physical_key: Option<Key>) -> bool {
        self.key == key && self.physical_key == physical_key
    }

    fn resolve_text(&mut self, text: &str, mode: TermMode) -> InputEmission {
        if self
            .press_translation
            .as_ref()
            .and_then(|translation| translation.suppress_text.as_deref())
            .is_some_and(|expected| expected == text)
        {
            return InputEmission::pty(self.flush_fallback());
        }

        if let Some(translation) = input::translate_text_event(
            input::KeyIdentity::new(self.key, self.physical_key, self.key_without_modifiers_text.as_deref()),
            text,
            input::KeyEventContext::new(true, false, self.modifiers, mode),
        ) {
            let mut bytes = translation.bytes;
            if self.release_seen
                && let Some(release) = input::translate_text_event(
                    input::KeyIdentity::new(self.key, self.physical_key, self.key_without_modifiers_text.as_deref()),
                    text,
                    input::KeyEventContext::new(false, false, self.modifiers, mode),
                )
            {
                bytes.extend_from_slice(&release.bytes);
            } else {
                self.synthetic_text = Some(text.to_owned());
            }
            return InputEmission::pty(bytes);
        }

        InputEmission::raw_text(text)
    }

    fn flush_fallback(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        if let Some(translation) = self.press_translation.as_ref() {
            bytes.extend_from_slice(&translation.bytes);
        }
        if self.release_seen
            && let Some(translation) = self.release_translation.as_ref()
        {
            bytes.extend_from_slice(&translation.bytes);
        }
        bytes
    }
}

#[derive(Default)]
struct InputEmission {
    bytes: Vec<u8>,
    clears_selection: bool,
}

impl InputEmission {
    fn pty(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            clears_selection: false,
        }
    }

    fn raw_text(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
            clears_selection: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyboardInputForwarder, TerminalInputEvent, disconnected_ssh_reconnect_requested};
    use alacritty_terminal::term::TermMode;
    use egui::{Event, Key, Modifiers};
    use horizon_core::{PanelKind, SshConnectionStatus};

    #[test]
    fn disconnected_ssh_panels_request_reconnect_from_local_shortcut() {
        assert!(disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Disconnected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn connected_ssh_panels_ignore_local_reconnect_shortcut() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Connected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn non_ssh_panels_ignore_local_reconnect_shortcut() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Shell,
            None,
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                false,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn repeated_reconnect_shortcut_does_not_queue_another_restart() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Disconnected),
            &[key_event(
                Key::R,
                Some(Key::R),
                None,
                true,
                true,
                Modifiers::COMMAND | Modifiers::SHIFT,
            )],
        ));
    }

    #[test]
    fn altgr_text_after_release_stays_on_text_path_without_report_all_keys() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::ALT),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::ALT),
            text_event("@"),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn shifted_symbol_uses_text_reconciliation_without_forcing_kitty_sequences() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::SHIFT),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::SHIFT),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn plain_space_stays_on_text_path_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b" ");
    }

    #[test]
    fn repeated_spaces_do_not_get_dropped_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::NONE),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"  ");
    }

    #[test]
    fn shifted_space_stays_on_text_path_in_kitty_basic_mode() {
        let events = vec![
            key_event(Key::Space, Some(Key::Space), Some(" "), true, false, Modifiers::SHIFT),
            text_event(" "),
            key_event(Key::Space, Some(Key::Space), Some(" "), false, false, Modifiers::SHIFT),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b" ");
    }

    /// Regression: on some Linux setups, `AltGr` is NOT reported as
    /// `modifiers.alt` by winit. The key event must not leak the base
    /// key ("2") ahead of the later text event ("@"), even when kitty
    /// keyboard mode is active.
    #[test]
    fn altgr_without_alt_modifier_in_kitty_mode_does_not_leak_base_key() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::NONE),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, b"@");
    }

    /// Same scenario as above but in non-kitty mode: the text event
    /// should pass through as raw "@" with no preceding "2".
    #[test]
    fn altgr_without_alt_modifier_in_legacy_mode_emits_only_text() {
        let events = vec![
            key_event(Key::Num2, Some(Key::Num2), Some("2"), true, false, Modifiers::NONE),
            text_event("@"),
            key_event(Key::Num2, Some(Key::Num2), Some("2"), false, false, Modifiers::NONE),
        ];

        let bytes = forward_bytes(&events, TermMode::NONE);

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn shifted_international_key_stays_on_text_path_without_report_all_keys() {
        let events = vec![
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                true,
                false,
                Modifiers::SHIFT,
            ),
            text_event("Å"),
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                false,
                false,
                Modifiers::SHIFT,
            ),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
        );

        assert_eq!(bytes, "Å".as_bytes());
    }

    #[test]
    fn report_all_keys_keeps_printable_text_on_kitty_sequence_path() {
        let events = vec![
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                true,
                false,
                Modifiers::SHIFT,
            ),
            text_event("Å"),
            key_event(
                Key::OpenBracket,
                Some(Key::OpenBracket),
                Some("å"),
                false,
                false,
                Modifiers::SHIFT,
            ),
        ];

        let bytes = forward_bytes(
            &events,
            TermMode::DISAMBIGUATE_ESC_CODES
                | TermMode::REPORT_EVENT_TYPES
                | TermMode::REPORT_ALTERNATE_KEYS
                | TermMode::REPORT_ALL_KEYS_AS_ESC,
        );

        assert_eq!(bytes, b"\x1b[229:197:91;2u\x1b[229:197:91;2:3u");
    }

    #[test]
    fn legacy_c0_key_events_are_forwarded_in_legacy_mode() {
        let cases: [(&str, TerminalInputEvent, &[u8]); 6] = [
            (
                "shift enter",
                key_event(Key::Enter, Some(Key::Enter), None, true, false, Modifiers::SHIFT),
                b"\r",
            ),
            (
                "alt escape",
                key_event(Key::Escape, Some(Key::Escape), None, true, false, Modifiers::ALT),
                b"\x1b\x1b",
            ),
            (
                "ctrl backspace",
                key_event(Key::Backspace, Some(Key::Backspace), None, true, false, Modifiers::CTRL),
                b"\x08",
            ),
            (
                "alt backspace",
                key_event(Key::Backspace, Some(Key::Backspace), None, true, false, Modifiers::ALT),
                b"\x1b\x7f",
            ),
            (
                "ctrl shift tab",
                key_event(
                    Key::Tab,
                    Some(Key::Tab),
                    None,
                    true,
                    false,
                    Modifiers::CTRL | Modifiers::SHIFT,
                ),
                b"\x1b[Z",
            ),
            (
                "alt shift tab",
                key_event(
                    Key::Tab,
                    Some(Key::Tab),
                    None,
                    true,
                    false,
                    Modifiers::ALT | Modifiers::SHIFT,
                ),
                b"\x1b\x1b[Z",
            ),
        ];

        for (name, event, expected) in cases {
            let bytes = forward_bytes(&[event], TermMode::NONE);
            assert_eq!(bytes, expected, "{name}");
        }
    }

    fn forward_bytes(events: &[TerminalInputEvent], mode: TermMode) -> Vec<u8> {
        let mut forwarder = KeyboardInputForwarder::default();
        let mut bytes = Vec::new();

        for event in events {
            let emission = match &event.event {
                Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => forwarder.on_text(text, mode),
                Event::Key { .. } => forwarder.on_key(event, mode),
                _ => continue,
            };
            bytes.extend_from_slice(&emission.bytes);
        }

        bytes.extend_from_slice(&forwarder.finish().bytes);
        bytes
    }

    fn key_event(
        key: Key,
        physical_key: Option<Key>,
        key_without_modifiers_text: Option<&str>,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
    ) -> TerminalInputEvent {
        TerminalInputEvent {
            event: Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            },
            key_without_modifiers_text: key_without_modifiers_text.map(ToOwned::to_owned),
            observed_key: None,
        }
    }

    fn text_event(text: &str) -> TerminalInputEvent {
        TerminalInputEvent {
            event: Event::Text(text.to_owned()),
            key_without_modifiers_text: None,
            observed_key: None,
        }
    }
}
