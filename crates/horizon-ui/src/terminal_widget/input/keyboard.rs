use std::collections::VecDeque;

use alacritty_terminal::term::TermMode;
use egui::Key;
use horizon_core::{Panel, PanelKind, ShortcutBinding, ShortcutKey, ShortcutModifiers, SshConnectionStatus};

use super::super::ime::{prepare_terminal_keyboard_events, store_terminal_ime_enabled, terminal_ime_enabled};
use crate::app::shortcuts::shortcut_event_matches;
use crate::clipboard_image::ClipboardImages;
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
    clipboard_images: &ClipboardImages,
    local_ssh_reconnect_enabled: bool,
) -> bool {
    if local_ssh_reconnect_enabled
        && disconnected_ssh_reconnect_requested(panel.kind, panel.ssh_status(), panel.attaches_a_tmux_session(), events)
    {
        return true;
    }

    // A panel whose program ended takes no key. The line under its grid says
    // that typed text is not sent, so the key reaches no dead PTY and no key is
    // dropped without that sentence.
    if !panel.accepts_input() {
        return false;
    }

    let panel_id = panel.id;
    let Some(terminal) = panel.terminal_mut() else {
        return false;
    };
    // The clock of the latency lines, taken once for every key event that this
    // frame handles. It is `None` while the switch is off.
    let key_at = terminal.press_clock();
    let mode = terminal.mode();
    let mut forwarder = KeyboardInputForwarder::default();
    let mut ime_enabled = terminal_ime_enabled(ui, terminal_id);
    let events = prepare_terminal_keyboard_events(events, ime_enabled);

    for event in &events {
        match &event.event {
            // A non-empty preedit marks an active composition; an empty
            // preedit means the IME was dismissed.
            egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                ime_enabled = !text.is_empty();
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
                    write_keys(terminal, key_at, &emission.bytes);
                }
            }
            egui::Event::Paste(text) => {
                terminal.clear_selection();
                let bytes = input::paste_bytes(text, mode, true);
                write_keys(terminal, key_at, &bytes);
            }
            // A terminal takes text, so a clipboard image becomes a private PNG
            // file and its path is pasted when the worker has written it.
            egui::Event::PasteImage(image) => {
                clipboard_images.persist(ui.ctx(), panel_id, image.clone());
            }
            egui::Event::Copy => {
                if event.is_plain_ctrl_c_copy_command() {
                    write_keys(terminal, key_at, &[3]);
                } else if let Some(text) = take_selection_for_copy(terminal) {
                    primary_selection.copy(&text);
                    crate::clipboard::copy_text(text);
                }
            }
            egui::Event::Cut => {
                if let Some(text) = take_selection_for_copy(terminal) {
                    primary_selection.copy(&text);
                    crate::clipboard::copy_text(text);
                }
                if input::cut_chord_is_control_character() {
                    write_keys(terminal, key_at, &[24]);
                }
            }
            // A clipboard or scrollback chord belongs to the terminal window,
            // not to the program in the PTY. Every press, repeat and release of
            // such a chord is consumed here; a release alone would otherwise
            // reach the PTY as a modified key sequence.
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                if let Some(command) = input::terminal_local_command(*key, *physical_key, *modifiers) {
                    if *pressed && !*repeat {
                        run_terminal_local_command(ui, terminal, primary_selection, command);
                    }
                } else {
                    let emission = forwarder.on_key(event, mode);
                    if !emission.bytes.is_empty() {
                        write_keys(terminal, key_at, &emission.bytes);
                    }
                }
            }
            _ => {}
        }
    }

    let emission = forwarder.finish();
    if !emission.bytes.is_empty() {
        write_keys(terminal, key_at, &emission.bytes);
    }

    store_terminal_ime_enabled(ui, terminal_id, ime_enabled);

    false
}

/// Writes the bytes of one input of a person into the terminal of this panel.
///
/// `key_at` is the time that the frame took the key event, so the opt-in latency
/// line of that press starts where the person's key reached the window.
///
/// The write arms the echo of a panel that this frame draws, before the bytes of
/// the press leave, so the echo asks for the next frame instead of waiting for
/// the idle poll of the window. Only bytes that reach the PTY arm that wake, so a
/// chord that the window handles and output that nobody typed wait as before.
fn write_keys(terminal: &horizon_core::Terminal, key_at: Option<std::time::Instant>, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    terminal.arm_output_wake();
    terminal.write_input(bytes);
    if let Some(key_at) = key_at {
        terminal.note_key_press(key_at, bytes.len());
    }
}

/// Run a chord that the terminal window owns. Nothing here reaches the PTY.
fn run_terminal_local_command(
    ui: &egui::Ui,
    terminal: &mut horizon_core::Terminal,
    primary_selection: &PrimarySelection,
    command: input::TerminalLocalCommand,
) {
    match command {
        input::TerminalLocalCommand::Copy => {
            if let Some(text) = take_selection_for_copy(terminal) {
                primary_selection.copy(&text);
                crate::clipboard::copy_text(text);
            }
        }
        // The toolkit reads the desktop clipboard for this viewport, so the text
        // and the image arrive on the same events as the toolkit's own paste
        // chord. Applied clipboard images reach the terminal as a file path.
        input::TerminalLocalCommand::Paste => {
            super::super::clipboard_paste::request(ui.ctx());
        }
        input::TerminalLocalCommand::ScrollLines(lines) => terminal.scroll_scrollback_by(lines),
        input::TerminalLocalCommand::ScrollPage(pages) => {
            // The last line of the old page stays visible as the first line of
            // the new one, as it does in GNOME Terminal.
            let lines_per_page = i32::from(terminal.rows().saturating_sub(1)).max(1);
            terminal.scroll_scrollback_by(lines_per_page.saturating_mul(pages));
        }
        input::TerminalLocalCommand::ScrollToTop => terminal.set_scrollback(terminal.history_size()),
        input::TerminalLocalCommand::ScrollToBottom => terminal.set_scrollback(0),
    }
}

/// The selected text, ending the selection either way so a drag over blank
/// cells does not stay highlighted after a copy that had nothing to copy.
fn take_selection_for_copy(terminal: &horizon_core::Terminal) -> Option<String> {
    let text = terminal.selection_to_string();
    terminal.clear_selection();
    text
}

fn disconnected_ssh_reconnect_requested(
    kind: PanelKind,
    ssh_status: Option<SshConnectionStatus>,
    machine_tied: bool,
    events: &[TerminalInputEvent],
) -> bool {
    if kind != PanelKind::Ssh {
        return false;
    }
    match ssh_status {
        // A disconnected panel keeps its last screen, so the shortcut asks for
        // a connection.
        Some(SshConnectionStatus::Disconnected) => {
            pressed_keys(events).any(|input_event| shortcut_event_matches(&input_event.event, SSH_RECONNECT_SHORTCUT))
        }
        // A panel that waits for its next attempt has no shell to type into, so
        // any key asks for the connection now instead of waiting.
        Some(SshConnectionStatus::Reconnecting) => pressed_keys(events).next().is_some(),
        // An attempt of a machine terminal runs its own `ssh`, so the text that a
        // person types during `Connecting...` goes into that attempt and reaches
        // the shell when the link is up. Only the two keys that ask for something
        // end the attempt: `Esc` stops it and `Enter` says "do it now". A letter
        // never ends it, because a person who types ahead on a link of 300 ms
        // would otherwise keep cancelling a connect that is only slow.
        Some(SshConnectionStatus::Connecting) if machine_tied => pressed_keys(events).any(|input_event| {
            matches!(
                &input_event.event,
                egui::Event::Key {
                    key: Key::Enter | Key::Escape,
                    ..
                }
            )
        }),
        Some(SshConnectionStatus::Connecting | SshConnectionStatus::Connected) | None => false,
    }
}

/// The key presses of one pass that are not a repeat of a held key.
fn pressed_keys(events: &[TerminalInputEvent]) -> impl Iterator<Item = &TerminalInputEvent> {
    events.iter().filter(|input_event| {
        matches!(
            &input_event.event,
            egui::Event::Key {
                pressed: true,
                repeat: false,
                ..
            }
        )
    })
}

struct KeyboardInputForwarder {
    macos: bool,
    suppressed_text: VecDeque<String>,
    deferred_text_key: Option<DeferredTextKey>,
}

impl Default for KeyboardInputForwarder {
    fn default() -> Self {
        Self::for_system(cfg!(target_os = "macos"))
    }
}

impl KeyboardInputForwarder {
    /// The forwarder of the system that `macos` names, so that a test reads the
    /// keys of both systems on either one.
    fn for_system(macos: bool) -> Self {
        Self {
            macos,
            suppressed_text: VecDeque::new(),
            deferred_text_key: None,
        }
    }

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
                deferred.release_translation = input::translate_key_event_on(
                    key_identity,
                    input::KeyEventContext::new(false, *repeat, *modifiers, mode),
                    self.macos,
                );
                return emission;
            } else if !deferred.matches(*key, *physical_key) {
                emission.bytes.extend_from_slice(&deferred.flush_fallback());
                self.deferred_text_key = None;
            }
        }

        if let Some(translation) = input::translate_key_event_on(key_identity, context, self.macos) {
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
    use super::{
        KeyboardInputForwarder, TerminalInputEvent, disconnected_ssh_reconnect_requested, take_selection_for_copy,
    };
    use alacritty_terminal::term::TermMode;
    use egui::{Event, Key, Modifiers};
    use horizon_core::{PanelKind, SshConnectionStatus};

    #[test]
    fn remapped_function_key_emits_only_logical_unicode() {
        let events = [
            key_event(Key::F19, Some(Key::F19), Some("æ"), true, false, Modifiers::NONE),
            text_event("æ"),
            key_event(Key::F19, Some(Key::F19), Some("æ"), false, false, Modifiers::NONE),
        ];
        assert_eq!(forward_bytes(&events, TermMode::NONE), "æ".as_bytes());
        assert_eq!(forward_bytes(&events, TermMode::REPORT_ALL_KEYS_AS_ESC), b"\x1b[230u");
        assert_eq!(
            forward_bytes(&events, TermMode::REPORT_ALL_KEYS_AS_ESC | TermMode::REPORT_EVENT_TYPES),
            b"\x1b[230u\x1b[230;1:3u"
        );
    }

    #[test]
    fn genuine_function_key_retains_modified_and_kitty_sequences() {
        let events = [key_event(Key::F19, Some(Key::F19), None, true, false, Modifiers::CTRL)];
        assert_eq!(forward_bytes(&events, TermMode::NONE), b"\x1b[33;5~");
        assert_eq!(
            forward_bytes(&events, TermMode::REPORT_ALL_KEYS_AS_ESC),
            b"\x1b[57382;5u"
        );
    }

    #[test]
    fn disconnected_ssh_panels_request_reconnect_from_local_shortcut() {
        assert!(disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Disconnected),
            false,
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
    fn a_terminal_that_waits_for_its_attempt_reconnects_on_any_key() {
        assert!(disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Reconnecting),
            true,
            &[key_event(Key::A, Some(Key::A), Some("a"), true, false, Modifiers::NONE)],
        ));
    }

    #[test]
    fn a_repeated_key_of_a_waiting_terminal_starts_nothing() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Reconnecting),
            true,
            &[key_event(Key::A, Some(Key::A), Some("a"), true, true, Modifiers::NONE)],
        ));
    }

    #[test]
    fn typing_in_a_machine_terminal_that_connects_keeps_the_attempt() {
        // A letter of a person who types ahead goes into the attempt that runs, so
        // it reaches the shell when the link is up. At a round trip of 300 ms a
        // connect takes seconds, and a person who types during it must not cancel
        // it again and again.
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Connecting),
            true,
            &[key_event(Key::A, Some(Key::A), Some("a"), true, false, Modifiers::NONE)],
        ));
        assert!(
            !disconnected_ssh_reconnect_requested(
                PanelKind::Ssh,
                Some(SshConnectionStatus::Connecting),
                true,
                &[key_event(
                    Key::Space,
                    Some(Key::Space),
                    Some(" "),
                    true,
                    false,
                    Modifiers::NONE
                )],
            ),
            "a space is text too"
        );
    }

    #[test]
    fn escape_or_enter_ends_the_attempt_of_a_machine_terminal_that_is_in_flight() {
        // `Esc` and `Enter` are the two keys that ask for something, so they also
        // end an attempt that a person does not want to wait for. The rule cannot
        // depend on the time, because the same key would then do different things
        // under the hands of the person.
        for key in [Key::Escape, Key::Enter] {
            assert!(
                disconnected_ssh_reconnect_requested(
                    PanelKind::Ssh,
                    Some(SshConnectionStatus::Connecting),
                    true,
                    &[key_event(key, Some(key), None, true, false, Modifiers::NONE)],
                ),
                "{key:?} asks for the attempt now"
            );
            assert!(
                !disconnected_ssh_reconnect_requested(
                    PanelKind::Ssh,
                    Some(SshConnectionStatus::Connecting),
                    true,
                    &[key_event(key, Some(key), None, true, true, Modifiers::NONE)],
                ),
                "a held {key:?} starts one attempt, not one for each repeat"
            );
        }
        assert!(
            !disconnected_ssh_reconnect_requested(
                PanelKind::Ssh,
                Some(SshConnectionStatus::Connecting),
                false,
                &[key_event(
                    Key::Escape,
                    Some(Key::Escape),
                    None,
                    true,
                    false,
                    Modifiers::NONE
                )],
            ),
            "a plain ssh panel that runs a silent command keeps its attempt"
        );
    }

    #[test]
    fn connected_ssh_panels_ignore_local_reconnect_shortcut() {
        assert!(!disconnected_ssh_reconnect_requested(
            PanelKind::Ssh,
            Some(SshConnectionStatus::Connected),
            true,
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
            false,
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
            false,
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

    /// The events that a Mac reports for the Option key of a Norwegian layout,
    /// taken from a trace of that system: `(key, physical key, the text of the
    /// key without modifiers, the character that the layout composes)`.
    const OPTION_COMPOSED_ON_A_MAC: [(Key, Key, &str, &str); 5] = [
        (Key::Pipe, Key::Num7, "7", "|"),
        (Key::OpenBracket, Key::Num8, "8", "["),
        (Key::CloseBracket, Key::Num9, "9", "]"),
        (Key::Quote, Key::Backslash, "@", "'"),
        (Key::B, Key::B, "b", "∫"),
    ];

    /// The events that a Linux system reports for a chord of Alt and Shift on a
    /// US layout: `(key, physical key, the text of the key without modifiers,
    /// the text of the chord)`. The toolkit reports the shifted symbol as a key
    /// of its own when it has one, and the physical key otherwise.
    const ALT_SHIFT_ON_LINUX: [(Key, Key, &str, &str); 10] = [
        (Key::Questionmark, Key::Slash, "/", "?"),
        (Key::Exclamationmark, Key::Num1, "1", "!"),
        (Key::Colon, Key::Semicolon, ";", ":"),
        (Key::Pipe, Key::Backslash, "\\", "|"),
        (Key::OpenCurlyBracket, Key::OpenBracket, "[", "{"),
        (Key::CloseCurlyBracket, Key::CloseBracket, "]", "}"),
        (Key::Plus, Key::Equals, "=", "+"),
        (Key::Minus, Key::Minus, "-", "_"),
        (Key::Comma, Key::Comma, ",", "<"),
        (Key::Period, Key::Period, ".", ">"),
    ];

    /// The press, the text that follows it and the release of one chord with
    /// Alt, and with Shift when `shift` is set.
    fn chord_events(
        key: Key,
        physical_key: Key,
        key_without_modifiers_text: &str,
        text: &str,
        shift: bool,
    ) -> Vec<TerminalInputEvent> {
        let modifiers = Modifiers {
            alt: true,
            shift,
            ..Modifiers::NONE
        };
        vec![
            key_event(
                key,
                Some(physical_key),
                Some(key_without_modifiers_text),
                true,
                false,
                modifiers,
            ),
            text_event(text),
            key_event(
                key,
                Some(physical_key),
                Some(key_without_modifiers_text),
                false,
                false,
                modifiers,
            ),
        ]
    }

    /// The Alt key of Linux stays the Meta key for every chord of Shift, also
    /// where the toolkit reports the shifted symbol as a key of its own. These
    /// are the bytes of the head before the rule of Option: the text of a symbol
    /// with Shift is not printable text, so the Meta path of these chords writes
    /// no `ESC` on either head, and the text of the key reaches the terminal.
    #[test]
    fn alt_with_shift_keeps_the_bytes_of_linux() {
        for (key, physical_key, without_modifiers, text) in ALT_SHIFT_ON_LINUX {
            let events = chord_events(key, physical_key, without_modifiers, text, true);

            assert_eq!(
                forward_bytes_on(&events, TermMode::NONE, false),
                text.as_bytes(),
                "Alt+Shift with {key:?} gives {text} alone on Linux, as it did before the rule of Option"
            );
        }
    }

    #[test]
    fn option_composed_symbols_reach_the_pty_as_their_own_character() {
        for (key, physical_key, without_option, composed) in OPTION_COMPOSED_ON_A_MAC {
            let events = chord_events(key, physical_key, without_option, composed, false);

            assert_eq!(
                forward_bytes_on(&events, TermMode::NONE, true),
                composed.as_bytes(),
                "{composed} is the character of the layout, not Meta and {key:?}"
            );
        }
    }

    #[test]
    fn option_without_a_composed_character_stays_the_meta_key() {
        let events = chord_events(Key::A, Key::A, "a", "a", false);

        assert_eq!(forward_bytes_on(&events, TermMode::NONE, true), b"\x1ba");
    }

    #[test]
    fn option_with_shift_keeps_a_letter_on_the_meta_path() {
        let events = chord_events(Key::A, Key::A, "a", "A", true);

        assert_eq!(forward_bytes_on(&events, TermMode::NONE, true), b"\x1bA");
    }

    /// The layout composes a character for Option and Shift together as well, so
    /// the terminal takes that character: Option+Shift+7 is `\` on a Norwegian
    /// layout.
    #[test]
    fn option_with_shift_composes_a_symbol_of_the_layout() {
        let events = chord_events(Key::Backslash, Key::Num7, "7", "\\", true);

        assert_eq!(forward_bytes_on(&events, TermMode::NONE, true), b"\\");
    }

    /// The kitty keyboard modes read the key before the rule of Option, so they
    /// report the same bytes on both systems.
    #[test]
    fn a_kitty_mode_reports_alt_with_shift_as_it_did() {
        let events = chord_events(Key::Questionmark, Key::Slash, "/", "?", true);
        let letter = chord_events(Key::A, Key::A, "a", "A", true);

        for macos in [true, false] {
            assert_eq!(
                forward_bytes_on(
                    &events,
                    TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES | TermMode::REPORT_ALTERNATE_KEYS,
                    macos,
                ),
                b"?",
                "a symbolic key of Alt and Shift reports its text in a kitty mode, macos={macos}"
            );
            assert_eq!(
                forward_bytes_on(&letter, TermMode::REPORT_ALL_KEYS_AS_ESC, macos),
                b"\x1b[97;4u",
                "a letter of Alt and Shift reports its kitty sequence, macos={macos}"
            );
        }
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

    fn snapshot_panel(replay: &str) -> (horizon_core::Panel, tempfile::TempDir) {
        let transcript_root = tempfile::tempdir().expect("transcript tempdir");
        std::fs::write(transcript_root.path().join("copy-panel.bin"), replay).expect("write transcript");
        let panel = horizon_core::Panel::spawn(
            horizon_core::PanelId(1),
            horizon_core::WorkspaceId(1),
            horizon_core::PanelOptions {
                kind: PanelKind::Ssh,
                rows: 8,
                cols: 25,
                local_id: Some("copy-panel".to_string()),
                transcript_root: Some(transcript_root.path().to_path_buf()),
                restore_as_disconnected_snapshot: true,
                ..horizon_core::PanelOptions::default()
            },
        )
        .expect("spawn disconnected snapshot");
        (panel, transcript_root)
    }

    #[test]
    fn copying_selected_text_returns_it_and_ends_the_selection() {
        let (panel, _transcript_root) = snapshot_panel("copy me");
        let terminal = panel.terminal().expect("terminal");
        terminal.start_selection(
            horizon_core::SelectionType::Simple,
            0,
            0,
            horizon_core::TerminalSide::Left,
        );
        terminal.update_selection(0, 6, horizon_core::TerminalSide::Right);

        assert_eq!(take_selection_for_copy(terminal).as_deref(), Some("copy me"));
        assert!(!terminal.has_selection());
    }

    #[test]
    fn copying_a_blank_selection_copies_nothing_and_ends_the_selection() {
        let (panel, _transcript_root) = snapshot_panel("copy me");
        let terminal = panel.terminal().expect("terminal");
        terminal.start_selection(
            horizon_core::SelectionType::Simple,
            5,
            0,
            horizon_core::TerminalSide::Left,
        );
        terminal.update_selection(5, 12, horizon_core::TerminalSide::Right);
        assert!(terminal.has_selection());

        assert_eq!(take_selection_for_copy(terminal), None);
        assert!(!terminal.has_selection());
    }

    fn forward_bytes(events: &[TerminalInputEvent], mode: TermMode) -> Vec<u8> {
        forward_bytes_on(events, mode, cfg!(target_os = "macos"))
    }

    fn forward_bytes_on(events: &[TerminalInputEvent], mode: TermMode, macos: bool) -> Vec<u8> {
        let mut forwarder = KeyboardInputForwarder::for_system(macos);
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
