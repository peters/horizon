//! Speech input: a hotkey, one take, and the insert of the text of that take.
//!
//! Horizon holds no speech engine, no audio library and no model. It runs a
//! recorder that the person named in the settings while the hotkey is down, and it
//! sends the recording to a transcription service over HTTPS or to a transcription
//! command. The text of that answer goes into the panel that had the focus, through
//! the paste path of the terminal.
//!
//! The pieces of this module:
//!
//! - [`command`] splits one command line of the settings into arguments, with the
//!   placeholder `{file}`. No shell reads the line.
//! - [`endpoint`] checks the endpoint of a service: `https`, or `http` on a loopback
//!   address.
//! - [`service`] builds the multipart body of one request and reads one answer.
//! - [`text`] cleans the text of an answer: no line break, no tab and no other
//!   control character, trimmed, cut at 8 KiB.
//!
//! `docs/review/trust-boundaries/speech-input.md` states what is untrusted here,
//! what leaves the computer, what this module guarantees and what it does not.

pub mod command;
pub mod endpoint;
pub mod service;
pub mod text;

pub use command::{CommandLine, CommandLineError, FILE_PLACEHOLDER};
pub use endpoint::{Endpoint, EndpointError};
pub use text::{MAX_TEXT_BYTES, clean_text};

/// One operating system that this build supports, where a default of speech input
/// differs. The value is a value and not a `cfg!` at the call site, so both branches
/// are tested on one platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeechPlatform {
    Linux,
    MacOs,
}

impl SpeechPlatform {
    /// The platform of this build.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The recorder line that the settings hold until a person changes it.
    ///
    /// PipeWire gives a recording of 16 kHz on one channel in the format that every
    /// transcription service reads. macOS has no recorder command of the system, so
    /// the line stays empty and the documents give two recipes that a person
    /// installs.
    #[must_use]
    pub const fn default_recorder(self) -> &'static str {
        match self {
            Self::Linux => "pw-record --rate 16000 --channels 1 --format s16 {file}",
            Self::MacOs => "",
        }
    }

    /// The name of this platform, for a document and the Settings tab.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpeechPlatform;

    #[test]
    fn the_default_recorder_of_each_platform_names_its_program() {
        // Both branches are read on one platform, so the macOS one is tested here
        // too, and the macOS lane of the procedure confirms it on that platform.
        assert_eq!(
            SpeechPlatform::Linux.default_recorder(),
            "pw-record --rate 16000 --channels 1 --format s16 {file}"
        );
        assert_eq!(SpeechPlatform::MacOs.default_recorder(), "");
        for platform in [SpeechPlatform::Linux, SpeechPlatform::MacOs] {
            let line = platform.default_recorder();
            assert!(
                line.is_empty() || line.contains(super::FILE_PLACEHOLDER),
                "{platform:?} names no take file"
            );
            assert!(!platform.name().is_empty());
        }
        assert_eq!(
            SpeechPlatform::current().name(),
            if cfg!(target_os = "macos") { "macOS" } else { "Linux" }
        );
    }
}
