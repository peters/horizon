//! Speech-to-text input: a per-panel mic button and a global push-to-talk
//! hotkey that dictate into a terminal panel as if the text had been typed.
//!
//! The whole subsystem is opt-in behind the `speech` cargo feature. Without
//! the feature this module compiles down to an inert stub with the same API,
//! so call sites stay free of `#[cfg]` noise. Audio is captured with cpal on
//! a dedicated thread, resampled to the 16 kHz mono f32 transcribe.cpp
//! expects, and transcribed on a worker thread that owns the model; results
//! flow back to the frame loop over mpsc channels (the same pattern as
//! `ssh_upload::worker`).

use std::fmt;

use horizon_core::{PanelId, SpeechConfig};

/// Opaque handle to a macOS Accessibility target retained on the UI thread.
///
/// The underlying AX objects never cross the speech worker boundary. This ID
/// is the only external-target state carried by the engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExternalTargetId(u64);

impl ExternalTargetId {
    #[must_use]
    pub(in crate::app) const fn from_raw(value: u64) -> Self {
        Self(value)
    }
}

/// Destination captured when a dictation starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeechTarget {
    Terminal(PanelId),
    External(ExternalTargetId),
}

/// Visual state of a panel's mic control.
#[cfg_attr(not(feature = "speech"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicState {
    Idle,
    Recording,
    /// Transcription in flight.
    Busy,
}

/// Events surfaced to the frame loop by [`SpeechSystem::poll`].
#[cfg_attr(not(feature = "speech"), allow(dead_code))]
pub enum SpeechEvent {
    /// Transcribed text ready to deliver to the target captured at key-down.
    Text {
        target: SpeechTarget,
        text: String,
    },
    /// A dictation attempt ended without text (too short, nothing heard,
    /// press ignored). Shown transiently so a silent outcome is never
    /// mistaken for dead hotkeys.
    Notice(String),
    Error(String),
}

/// Never expose transcript contents through incidental debug logging.
impl fmt::Debug for SpeechEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { target, .. } => formatter
                .debug_struct("Text")
                .field("target", target)
                .field("text", &"<redacted>")
                .finish(),
            Self::Notice(message) => formatter.debug_tuple("Notice").field(message).finish(),
            Self::Error(message) => formatter.debug_tuple("Error").field(message).finish(),
        }
    }
}

pub(crate) mod external_text;
pub(crate) mod global_hotkeys;
pub(crate) mod lifecycle;

#[cfg(feature = "speech")]
mod capture;
#[cfg(feature = "speech")]
mod engine;
#[cfg(feature = "speech")]
mod worker;
#[cfg(feature = "speech")]
pub use engine::SpeechSystem;

#[cfg(not(feature = "speech"))]
mod stub;
#[cfg(not(feature = "speech"))]
pub use stub::SpeechSystem;

/// Whether this binary was compiled with speech support.
#[must_use]
pub const fn built_with_speech() -> bool {
    cfg!(feature = "speech")
}

/// Preserve the committed global-hotkey intent while preventing a manager
/// from reserving system keys when capture/model worker startup failed.
#[must_use]
pub(in crate::app) fn global_hotkey_runtime_config(config: &SpeechConfig, runtime_available: bool) -> SpeechConfig {
    let mut runtime_config = config.clone();
    runtime_config.enabled &= runtime_available;
    runtime_config
}

/// Input devices the audio host reports, for the settings microphone
/// picker. Enumeration can block — keep it off the UI thread. Empty in
/// builds without the `speech` feature.
#[must_use]
pub fn list_input_devices() -> Vec<String> {
    #[cfg(feature = "speech")]
    {
        capture::list_input_devices()
    }
    #[cfg(not(feature = "speech"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::SpeechConfig;

    use super::{SpeechEvent, SpeechTarget, global_hotkey_runtime_config};

    #[test]
    fn text_event_debug_output_redacts_transcript() {
        let event = SpeechEvent::Text {
            target: SpeechTarget::Terminal(horizon_core::PanelId(7)),
            text: "private transcript".to_string(),
        };

        let debug = format!("{event:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("private transcript"));
    }

    #[test]
    fn unavailable_runtime_keeps_global_registration_off() {
        let config = SpeechConfig {
            enabled: true,
            dictate_outside_horizon: true,
            ..SpeechConfig::default()
        };

        let unavailable = global_hotkey_runtime_config(&config, false);
        assert!(!unavailable.enabled);
        assert!(unavailable.dictate_outside_horizon);
        assert_eq!(global_hotkey_runtime_config(&config, true), config);
    }
}
