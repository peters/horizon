//! Inert speech stub for builds without the `speech` cargo feature. Mirrors
//! the real [`super::SpeechSystem`] API so call sites need no `#[cfg]`.

// The stub keeps the real type's method receivers so call sites compile
// identically under both configurations.
#![allow(clippy::unused_self)]

use horizon_core::{PanelId, ShortcutBinding, SpeechConfig, SpeechHotkeyMode};

use super::{MicState, SpeechEvent};

pub struct SpeechSystem {}

impl SpeechSystem {
    /// Always `None`: speech was not compiled into this binary.
    #[must_use]
    pub fn from_config(_config: &SpeechConfig) -> Option<Self> {
        None
    }

    #[must_use]
    pub fn profile_bindings(&self) -> &[(usize, ShortcutBinding)] {
        &[]
    }

    #[must_use]
    pub fn hotkey_summary(&self, _primary_label: &str) -> Option<String> {
        None
    }

    #[must_use]
    pub fn hotkey_mode(&self) -> SpeechHotkeyMode {
        SpeechHotkeyMode::Hold
    }

    #[must_use]
    pub fn mic_state_for(&self, _panel: PanelId) -> MicState {
        MicState::Idle
    }

    #[must_use]
    pub fn recording_target(&self) -> Option<PanelId> {
        None
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        false
    }

    #[must_use]
    pub fn active_backend(&self) -> Option<&str> {
        None
    }

    pub fn toggle(&mut self, _target: PanelId) {}

    pub fn start(&mut self, _target: PanelId, _profile: usize) {}

    pub fn stop(&mut self) {}

    pub fn cancel(&mut self) {}

    pub fn poll(&mut self) -> Vec<SpeechEvent> {
        Vec::new()
    }
}
