//! The `speech` section of the settings: the hotkey, the recorder line and the
//! transcriber of speech input.
//!
//! Speech input is off while the recorder line is empty or while no transcriber is
//! set, and nothing of this section runs then. A person who keeps the recorder line
//! of the platform and stores the key of the service has the only settings that the
//! default flow needs.
//!
//! The section holds no value: `key_secret` is the *name* of a named secret, and the
//! value lives in the secrets file of this Horizon home, as the section `secrets`
//! says.

use serde::{Deserialize, Serialize};

use crate::named_secrets::SecretName;
use crate::shortcuts::{AppShortcuts, ShortcutBinding, ShortcutKey, ShortcutModifiers};
use crate::speech::{CommandLine, Endpoint, FILE_PLACEHOLDER, SpeechPlatform, TakePlan, Transcriber};

/// The hotkey of speech input until a person changes it.
pub const DEFAULT_SPEECH_HOTKEY: &str = "F9";

/// The endpoint of the service until a person changes it.
pub const DEFAULT_SPEECH_ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";

/// The model of the service until a person changes it.
pub const DEFAULT_SPEECH_MODEL: &str = "gpt-transcribe";

/// The named secret of the key until a person changes it. The names that start
/// with `OPENAI` belong to the tools of Horizon, so a named secret never takes one.
pub const DEFAULT_SPEECH_KEY_SECRET: &str = "SPEECH_API_KEY";

/// The longest language hint, in characters.
pub const MAX_LANGUAGE_CHARS: usize = 16;

/// Speech input: one hotkey, one recorder line and one transcriber.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct SpeechConfig {
    /// The hotkey that records while it is down.
    pub hotkey: String,
    /// One command line with `{file}`, which records while the hotkey is down.
    pub recorder: String,
    pub transcriber: TranscriberConfig,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_SPEECH_HOTKEY.to_owned(),
            recorder: SpeechPlatform::current().default_recorder().to_owned(),
            transcriber: TranscriberConfig::default(),
        }
    }
}

/// The transcriber of one take: one command of this computer, or one service.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct TranscriberConfig {
    pub kind: TranscriberKind,
    /// One command line with `{file}`, whose standard output is the text.
    pub command: String,
    /// The endpoint of the service.
    pub endpoint: String,
    pub model: String,
    /// The language hint, such as `no`, or empty for none.
    pub language: String,
    /// The name of the named secret that holds the key of the service.
    pub key_secret: String,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            kind: TranscriberKind::default(),
            command: String::new(),
            endpoint: DEFAULT_SPEECH_ENDPOINT.to_owned(),
            model: DEFAULT_SPEECH_MODEL.to_owned(),
            language: String::new(),
            key_secret: DEFAULT_SPEECH_KEY_SECRET.to_owned(),
        }
    }
}

/// Which transcriber one take uses.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriberKind {
    /// One command of this computer prints the text. Speech input is off while the
    /// line of that command is empty.
    #[default]
    Command,
    /// One service over HTTPS answers the text.
    Service,
}

impl TranscriberKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::Service => "Service",
        }
    }

    pub const ALL: &'static [Self] = &[Self::Command, Self::Service];
}

impl SpeechConfig {
    /// The take of this section, or `None` while speech input is off.
    ///
    /// A section that the rule refuses answers `None` as well: the Settings window
    /// shows why, and no take starts on a value that the rule refused.
    #[must_use]
    pub fn plan(&self) -> Option<TakePlan> {
        if self.recorder.trim().is_empty() {
            return None;
        }
        let recorder = CommandLine::parse(&self.recorder).ok()?;
        if !recorder.has_placeholder() {
            return None;
        }
        Some(TakePlan {
            recorder,
            transcriber: self.transcriber.plan()?,
        })
    }

    /// Whether speech input is on: a recorder and a transcriber are set.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.plan().is_some()
    }

    /// The sentence that says why speech input is off, or `None` while it is on.
    ///
    /// This reads the values and not the rule: a section that the rule refuses
    /// says so in [`Self::validate`], and the Settings tab shows both.
    #[must_use]
    pub fn off_reason(&self) -> Option<&'static str> {
        if self.is_on() {
            return None;
        }
        if self.recorder.trim().is_empty() {
            return Some("The recorder line is empty.");
        }
        if !CommandLine::parse(&self.recorder).is_ok_and(|line| line.has_placeholder()) {
            return Some("The recorder line needs {file}.");
        }
        match self.transcriber.kind {
            TranscriberKind::Command if self.transcriber.command.trim().is_empty() => {
                Some("Choose the service, or write the command that prints the text.")
            }
            TranscriberKind::Command
                if !CommandLine::parse(&self.transcriber.command).is_ok_and(|line| line.has_placeholder()) =>
            {
                Some("The transcriber command needs {file}.")
            }
            TranscriberKind::Service if self.key_secret().is_none() => {
                Some("The name of the named secret of the key is not one that Horizon keeps.")
            }
            TranscriberKind::Service => Some("The endpoint or the model of the service is not one that Horizon reads."),
            TranscriberKind::Command => Some("The transcriber command is not one that Horizon can run."),
        }
    }

    /// The name of the named secret that holds the key, when the service is the
    /// transcriber and the name passes the rule.
    #[must_use]
    pub fn key_secret(&self) -> Option<SecretName> {
        if self.transcriber.kind != TranscriberKind::Service {
            return None;
        }
        SecretName::parse(self.transcriber.key_secret.trim()).ok()
    }

    /// Why this section cannot be used, in the words of its rule.
    ///
    /// # Errors
    /// A hotkey that is not one shortcut, `Escape`, a key that a terminal needs, or
    /// a key of a global shortcut; a line that is not one command line or that has
    /// no `{file}`; an endpoint that is not `https` or `http` on a loopback
    /// address; an empty model; a language hint that is not one; and the name of a
    /// named secret that the rule refuses.
    pub fn validate(&self, shortcuts: &AppShortcuts) -> Result<(), String> {
        validate_hotkey(&self.hotkey, shortcuts)?;
        if !self.recorder.trim().is_empty() {
            check_line("speech.recorder", &self.recorder)?;
        }
        match self.transcriber.kind {
            TranscriberKind::Command => {
                if !self.transcriber.command.trim().is_empty() {
                    check_line("speech.transcriber.command", &self.transcriber.command)?;
                }
            }
            TranscriberKind::Service => {
                Endpoint::parse(&self.transcriber.endpoint)
                    .map_err(|error| format!("speech.transcriber.endpoint: {error}"))?;
                if self.transcriber.model.trim().is_empty() {
                    return Err("speech.transcriber.model is empty".to_owned());
                }
                SecretName::parse(self.transcriber.key_secret.trim())
                    .map_err(|error| format!("speech.transcriber.key_secret: {error}"))?;
                check_language(&self.transcriber.language)?;
            }
        }
        Ok(())
    }
}

impl TranscriberConfig {
    /// The transcriber of one take, or `None` while it is not set.
    #[must_use]
    pub fn plan(&self) -> Option<Transcriber> {
        match self.kind {
            TranscriberKind::Command => {
                let command = CommandLine::parse(&self.command).ok()?;
                if !command.has_placeholder() {
                    return None;
                }
                Some(Transcriber::Command(command))
            }
            TranscriberKind::Service => Some(Transcriber::Service {
                endpoint: Endpoint::parse(&self.endpoint).ok()?,
                model: self.model.trim().to_owned(),
                language: language_hint(&self.language),
                key_secret: SecretName::parse(self.key_secret.trim()).ok()?,
            }),
        }
    }
}

/// The language hint of the settings, or `None` when it is empty.
#[must_use]
pub fn language_hint(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn check_line(field: &str, line: &str) -> Result<(), String> {
    let line = CommandLine::parse(line).map_err(|error| format!("{field}: {error}"))?;
    if !line.has_placeholder() {
        return Err(format!("{field} has no {FILE_PLACEHOLDER}"));
    }
    Ok(())
}

fn check_language(language: &str) -> Result<(), String> {
    let language = language.trim();
    if language.is_empty() {
        return Ok(());
    }
    if language.chars().count() > MAX_LANGUAGE_CHARS
        || !language
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!(
            "speech.transcriber.language is not a hint of at most {MAX_LANGUAGE_CHARS} characters of A-Z, a-z, 0-9, - and _"
        ));
    }
    Ok(())
}

fn validate_hotkey(hotkey: &str, shortcuts: &AppShortcuts) -> Result<(), String> {
    let hotkey = hotkey.trim();
    if hotkey.is_empty() {
        return Ok(());
    }
    let binding = ShortcutBinding::parse(hotkey).map_err(|error| format!("speech.hotkey: {error}"))?;
    // Escape cancels the take that runs, so a binding of Escape would start and
    // cancel the same recording.
    if binding.key == ShortcutKey::Escape {
        return Err("speech.hotkey cannot use Escape, which cancels a take".to_owned());
    }
    // A bare typing or navigation key would be taken from every terminal panel
    // while it is down, so a bare key needs a modifier. A function key is a control
    // of its own.
    if binding.modifiers == ShortcutModifiers::NONE && !matches!(binding.key, ShortcutKey::Function(_)) {
        return Err(
            "speech.hotkey needs a modifier or a function key; a bare key would be taken from the terminal".to_owned(),
        );
    }
    if let Some((name, _)) = shortcuts
        .named_bindings()
        .into_iter()
        .find(|(_, global)| binding.overlaps(*global))
    {
        return Err(format!(
            "speech.hotkey overlaps the {name} shortcut; choose a key that nothing else uses"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Config, ShortcutsConfig};

    use super::{DEFAULT_SPEECH_KEY_SECRET, SpeechConfig, TranscriberKind};

    fn shortcuts() -> crate::shortcuts::AppShortcuts {
        ShortcutsConfig::default().resolve().expect("the default shortcuts")
    }

    #[test]
    fn the_default_section_holds_the_hotkey_and_no_transcriber() {
        let speech = SpeechConfig::default();
        assert_eq!(speech.hotkey, "F9");
        assert_eq!(speech.transcriber.kind, TranscriberKind::Command);
        assert_eq!(speech.transcriber.command, "");
        assert_eq!(speech.transcriber.endpoint, super::DEFAULT_SPEECH_ENDPOINT);
        assert_eq!(speech.transcriber.model, "gpt-transcribe");
        assert_eq!(speech.transcriber.key_secret, DEFAULT_SPEECH_KEY_SECRET);
        assert!(!speech.is_on(), "speech input is off until a transcriber is set");
        // The recorder of this build's platform is the default, and macOS has none.
        assert_eq!(
            speech.recorder,
            crate::speech::SpeechPlatform::current().default_recorder()
        );
        assert!(speech.validate(&shortcuts()).is_ok());
    }

    #[test]
    fn a_service_with_a_recorder_is_the_take_of_a_plan() {
        let mut speech = SpeechConfig {
            recorder: "rec --rate 16000 {file}".to_owned(),
            ..SpeechConfig::default()
        };
        speech.transcriber.kind = TranscriberKind::Service;
        let plan = speech.plan().expect("a take of the service");
        assert_eq!(plan.recorder.program(), "rec");
        match &plan.transcriber {
            crate::speech::Transcriber::Service { key_secret, model, .. } => {
                assert_eq!(key_secret.as_str(), DEFAULT_SPEECH_KEY_SECRET);
                assert_eq!(model, "gpt-transcribe");
            }
            crate::speech::Transcriber::Command(command) => {
                panic!("a service of a service kind: {command:?}")
            }
        }
    }

    #[test]
    fn speech_input_is_off_without_a_recorder_or_a_transcriber() {
        let mut speech = SpeechConfig {
            recorder: String::new(),
            ..SpeechConfig::default()
        };
        assert!(!speech.is_on());

        speech.recorder = "rec {file}".to_owned();
        assert!(!speech.is_on(), "an empty command line is no transcriber");

        speech.transcriber.command = "cat {file}".to_owned();
        assert!(speech.is_on());
    }

    #[test]
    fn a_hotkey_of_a_global_shortcut_and_a_bare_key_are_refused() {
        let mut speech = SpeechConfig::default();
        for (hotkey, expected) in [
            ("F11", "fullscreen_panel"),
            ("Ctrl+Shift+K", "command_palette"),
            ("Escape", "Escape"),
            ("Ctrl+Escape", "Escape"),
            ("K", "needs a modifier"),
            ("5", "needs a modifier"),
            ("Enter", "needs a modifier"),
            ("ArrowUp", "needs a modifier"),
            // A key that no global shortcut uses, with a modifier, is a hotkey.
        ] {
            speech.hotkey = hotkey.to_owned();
            let error = speech.validate(&shortcuts()).expect_err("a hotkey that is refused");
            assert!(error.contains(expected), "{hotkey}: {error}");
        }
        for hotkey in ["F9", "Ctrl+Alt+D", "Ctrl+Alt+ArrowUp", ""] {
            speech.hotkey = hotkey.to_owned();
            speech
                .validate(&shortcuts())
                .unwrap_or_else(|error| panic!("{hotkey}: {error}"));
        }
    }

    #[test]
    fn a_line_needs_the_placeholder_of_the_take_file() {
        let mut speech = SpeechConfig {
            recorder: "pw-record --rate 16000".to_owned(),
            ..SpeechConfig::default()
        };
        let error = speech.validate(&shortcuts()).expect_err("a line without a file");
        assert!(error.contains("has no {file}"), "{error}");

        speech.recorder = "pw-record --rate 16000 {file}".to_owned();
        speech.validate(&shortcuts()).expect("a line with a file");

        speech.recorder = "pw-record 'unterminated {file}".to_owned();
        let error = speech.validate(&shortcuts()).expect_err("a line inside a quote");
        assert!(error.contains("inside a quote"), "{error}");
    }

    #[test]
    fn the_endpoint_the_model_the_language_and_the_name_of_the_key_follow_their_rules() {
        let mut speech = SpeechConfig::default();
        speech.transcriber.kind = TranscriberKind::Service;

        speech.transcriber.endpoint = "http://api.example.test/v1".to_owned();
        let error = speech
            .validate(&shortcuts())
            .expect_err("a plain link off this computer");
        assert!(error.contains("http"), "{error}");
        speech.transcriber.endpoint = "http://127.0.0.1:8080/v1".to_owned();
        speech
            .validate(&shortcuts())
            .expect("a stand-in service on this computer");

        speech.transcriber.model = "  ".to_owned();
        let error = speech.validate(&shortcuts()).expect_err("no model");
        assert!(error.contains("model is empty"), "{error}");
        speech.transcriber.model = "gpt-transcribe".to_owned();

        for language in ["no no", "a\nb", "no!", "a-language-that-is-too-long"] {
            speech.transcriber.language = language.to_owned();
            let error = speech
                .validate(&shortcuts())
                .expect_err("a language hint that is refused");
            assert!(error.contains("language"), "{language}: {error}");
        }
        speech.transcriber.language = "nb-NO".to_owned();
        speech.validate(&shortcuts()).expect("a hint of a language");

        // The names that the tools of Horizon own cannot be a named secret.
        speech.transcriber.key_secret = "OPENAI_API_KEY".to_owned();
        let error = speech.validate(&shortcuts()).expect_err("a reserved name");
        assert!(error.contains("key_secret"), "{error}");
        assert!(speech.plan().is_none(), "a refused name starts no take");
        speech.transcriber.key_secret = "MY_SPEECH_KEY".to_owned();
        speech.validate(&shortcuts()).expect("a name of the person");
    }

    #[test]
    fn the_section_of_a_configuration_round_trips_through_json() {
        let mut config = Config::default();
        config.speech.hotkey = "Ctrl+Alt+S".to_owned();
        config.speech.transcriber.kind = TranscriberKind::Service;
        config.speech.transcriber.language = "no".to_owned();
        let json = config.to_json().expect("the settings as JSON");
        assert!(json.contains("\"speech\""), "{json}");
        let loaded = Config::from_json(&json).expect("the settings from JSON");
        assert_eq!(loaded.speech, config.speech);
        loaded.validate().expect("the section of this settings file");
    }
}
