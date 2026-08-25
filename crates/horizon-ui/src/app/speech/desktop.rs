//! Desktop-window dictation: sink selection, clipboard paste, global PTT.

use horizon_core::{PanelId, ShortcutBinding, ShortcutKey};
use horizon_cursor::{GlobalHotkeys, Hotkey, HotkeyError, HotkeyEvent, HotkeyKey, InjectError, send_paste_chord};

use super::super::HorizonApp;
use super::SpeechSink;

/// Choose the insert sink for a push-to-talk press.
///
/// A logically focused Horizon terminal (root or detached) is only used while
/// some Horizon window has OS focus. Otherwise a background instance would
/// keep routing global PTT into its last selected panel instead of the
/// focused external client.
#[must_use]
pub(crate) fn dictation_sink(
    focused_terminal: Option<PanelId>,
    desktop_injection: bool,
    horizon_focused: bool,
) -> Option<SpeechSink> {
    if desktop_injection && !horizon_focused {
        Some(SpeechSink::Desktop)
    } else {
        focused_terminal.map(SpeechSink::Panel)
    }
}

pub(crate) fn inject_desktop_transcript(text: &str) -> Result<(), InjectError> {
    if let Some(result) = take_test_inject_result(text) {
        return result;
    }
    let mut clipboard = arboard::Clipboard::new().map_err(|_| InjectError::Clipboard("clipboard unavailable"))?;
    clipboard
        .set_text(text)
        .map_err(|_| InjectError::Clipboard("failed to copy transcript"))?;
    send_paste_chord()
}

pub(crate) fn hotkey_from_binding(binding: ShortcutBinding) -> Option<Hotkey> {
    let key = match binding.key {
        ShortcutKey::Function(index) => HotkeyKey::Function(index),
        ShortcutKey::Letter(letter) => HotkeyKey::Letter(letter),
        ShortcutKey::Digit(digit) => HotkeyKey::Digit(digit),
        ShortcutKey::ArrowDown => HotkeyKey::ArrowDown,
        ShortcutKey::ArrowLeft => HotkeyKey::ArrowLeft,
        ShortcutKey::ArrowRight => HotkeyKey::ArrowRight,
        ShortcutKey::ArrowUp => HotkeyKey::ArrowUp,
        ShortcutKey::Enter => HotkeyKey::Enter,
        ShortcutKey::Tab => HotkeyKey::Tab,
        ShortcutKey::Comma => HotkeyKey::Comma,
        ShortcutKey::Minus => HotkeyKey::Minus,
        ShortcutKey::Plus => HotkeyKey::Plus,
        ShortcutKey::Escape => return None,
    };
    Some(Hotkey {
        ctrl: binding.modifiers.command() || binding.modifiers.ctrl(),
        shift: binding.modifiers.shift(),
        alt: binding.modifiers.alt(),
        super_: binding.modifiers.mac_cmd(),
        key,
    })
}

impl HorizonApp {
    pub(in crate::app) fn reset_speech_global_hotkeys(&mut self) {
        self.speech_global_hotkeys = None;
        self.speech_global_hotkeys_tried = false;
        self.speech_global_events_pending.clear();
    }

    pub(in crate::app) fn sync_speech_global_hotkeys(&mut self) {
        let enabled = self.template_config.features.speech.desktop_injection && self.speech.is_some();
        if !enabled {
            self.reset_speech_global_hotkeys();
            return;
        }
        if self.speech_global_hotkeys.is_some() || self.speech_global_hotkeys_tried {
            return;
        }
        let Some(speech) = self.speech.as_ref() else {
            return;
        };
        let bindings: Vec<(usize, Hotkey)> = speech
            .profile_bindings()
            .iter()
            .filter_map(|(profile, binding)| hotkey_from_binding(*binding).map(|hotkey| (*profile, hotkey)))
            .collect();
        if bindings.is_empty() {
            // Leave local egui PTT enabled; an empty listener would swallow it.
            self.speech_global_hotkeys = None;
            self.speech_global_hotkeys_tried = true;
            return;
        }
        self.speech_global_hotkeys_tried = true;
        match GlobalHotkeys::listen(&bindings) {
            Ok(hotkeys) => self.speech_global_hotkeys = Some(hotkeys),
            Err(HotkeyError::Unsupported) => {
                tracing::info!("desktop dictation: global hotkeys unavailable on this display");
            }
            Err(error) => {
                tracing::warn!(%error, "desktop dictation: failed to grab global push-to-talk keys");
            }
        }
    }
}

pub(crate) fn recv_global_hotkey(hotkeys: Option<&GlobalHotkeys>) -> Option<HotkeyEvent> {
    hotkeys.and_then(GlobalHotkeys::try_recv)
}

#[cfg(test)]
type InjectHook = fn(&str) -> Result<(), InjectError>;

#[cfg(test)]
static TEST_INJECT: std::sync::OnceLock<std::sync::Mutex<Option<InjectHook>>> = std::sync::OnceLock::new();

#[cfg(test)]
fn take_test_inject_result(text: &str) -> Option<Result<(), InjectError>> {
    TEST_INJECT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()?
        .map(|hook| hook(text))
}

#[cfg(not(test))]
fn take_test_inject_result(_text: &str) -> Option<Result<(), InjectError>> {
    None
}

#[cfg(test)]
pub(crate) fn set_test_inject_hook(hook: Option<InjectHook>) {
    *TEST_INJECT
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
}

#[cfg(test)]
mod tests {
    use horizon_core::{PanelId, ShortcutKey};

    use super::dictation_sink;
    use crate::app::speech::SpeechSink;

    #[test]
    fn focused_terminal_wins_over_desktop_flag_while_root_is_focused() {
        let panel = PanelId(3);
        assert_eq!(dictation_sink(Some(panel), true, true), Some(SpeechSink::Panel(panel)));
        assert_eq!(dictation_sink(Some(panel), false, true), Some(SpeechSink::Panel(panel)));
    }

    #[test]
    fn unfocused_horizon_uses_desktop_when_injection_is_enabled() {
        let panel = PanelId(3);
        assert_eq!(dictation_sink(Some(panel), true, false), Some(SpeechSink::Desktop));
        assert_eq!(
            dictation_sink(Some(panel), false, false),
            Some(SpeechSink::Panel(panel))
        );
    }

    #[test]
    fn a_focused_horizon_window_keeps_its_terminal_including_detached() {
        let panel = PanelId(9);
        assert_eq!(dictation_sink(Some(panel), true, true), Some(SpeechSink::Panel(panel)));
    }

    #[test]
    fn desktop_flag_fills_in_when_no_terminal_is_focused() {
        assert_eq!(dictation_sink(None, true, true), None);
        assert_eq!(dictation_sink(None, true, false), Some(SpeechSink::Desktop));
        assert_eq!(dictation_sink(None, false, true), None);
    }

    #[test]
    fn hotkey_from_binding_maps_every_accepted_speech_key() {
        let binding = |key| horizon_core::ShortcutBinding::new(horizon_core::ShortcutModifiers::CTRL, key);
        assert!(super::hotkey_from_binding(binding(ShortcutKey::ArrowUp)).is_some());
        assert!(super::hotkey_from_binding(binding(ShortcutKey::Enter)).is_some());
        assert!(super::hotkey_from_binding(binding(ShortcutKey::Escape)).is_none());
    }

    #[test]
    fn inject_hook_short_circuits_the_os_path() {
        super::set_test_inject_hook(Some(|_| Ok(())));
        assert!(super::inject_desktop_transcript("hello ").is_ok());
        super::set_test_inject_hook(Some(|_| Err(horizon_cursor::InjectError::Unsupported)));
        assert_eq!(
            super::inject_desktop_transcript("hello "),
            Err(horizon_cursor::InjectError::Unsupported)
        );
        super::set_test_inject_hook(None);
    }
}
