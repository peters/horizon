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

use horizon_core::PanelId;

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
#[derive(Debug)]
pub enum SpeechEvent {
    /// Transcribed text ready to inject into `target`'s PTY input.
    Text {
        target: PanelId,
        text: String,
    },
    /// A dictation attempt ended without text (too short, nothing heard,
    /// press ignored). Shown transiently so a silent outcome is never
    /// mistaken for dead hotkeys.
    Notice(String),
    Error(String),
}

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
