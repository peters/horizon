//! Speech-input configuration: profiles, task/backend enums, validation.
//!
//! Split from `config.rs` per the module-size guardrail; `config` re-exports
//! every public type so external paths are unchanged.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::shortcuts::{AppShortcuts, ShortcutBinding, ShortcutKey};

/// Speech-to-text input (push-to-talk dictation into the focused panel).
///
/// The config always parses so a `speech:` block never breaks a build made
/// without the `speech` cargo feature; the runtime simply ignores it there.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SpeechConfig {
    pub enabled: bool,
    /// Path to a transcribe.cpp GGUF model (e.g. nb-whisper-large-Q8_0.gguf).
    pub model: String,
    /// Source language hint (ISO code such as `no`, `nn`, `en`) or `auto`.
    pub language: String,
    pub task: SpeechTask,
    /// Destination language for the translate task (ISO code). The set the
    /// model actually supports comes from its GGUF metadata; whisper models
    /// translate to English only.
    pub target_language: String,
    pub backend: SpeechBackend,
    /// Push-to-talk shortcut in the shared shortcut syntax; empty disables.
    pub hotkey: String,
    pub hotkey_mode: SpeechHotkeyMode,
    /// Named speech profiles, each with its own push-to-talk key (e.g.
    /// F1 = Norwegian via NB-Whisper, F2 = English via whisper-large-v3).
    /// When empty, the flat fields above act as a single unnamed profile.
    pub profiles: Vec<SpeechProfile>,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: String::new(),
            language: "auto".to_string(),
            task: SpeechTask::Transcribe,
            target_language: "en".to_string(),
            backend: SpeechBackend::Auto,
            hotkey: "F9".to_string(),
            hotkey_mode: SpeechHotkeyMode::Hold,
            profiles: Vec::new(),
        }
    }
}

/// One dictation profile: a model plus its language/output settings and a
/// dedicated push-to-talk key. The key IS the language — holding F1 vs F2
/// needs no mode switching (Ventrilo-style channel binds).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SpeechProfile {
    pub name: String,
    /// Path to a transcribe.cpp GGUF model.
    pub model: String,
    /// Source language hint (ISO code) or `auto`.
    pub language: String,
    pub task: SpeechTask,
    pub target_language: String,
    /// Push-to-talk shortcut. The first profile is the mic-button default
    /// and may leave this empty; every later profile needs a hotkey to be
    /// reachable (the mic button reuses the last profile a hotkey started).
    pub hotkey: String,
}

impl Default for SpeechProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            model: String::new(),
            language: "auto".to_string(),
            task: SpeechTask::Transcribe,
            target_language: "en".to_string(),
            hotkey: String::new(),
        }
    }
}

impl SpeechConfig {
    /// The profiles to run: the explicit list, or a single profile
    /// synthesized from the flat fields for configs that predate profiles.
    #[must_use]
    pub fn resolved_profiles(&self) -> Vec<SpeechProfile> {
        if self.profiles.is_empty() {
            vec![SpeechProfile {
                name: "Default".to_string(),
                model: self.model.clone(),
                language: self.language.clone(),
                task: self.task,
                target_language: self.target_language.clone(),
                hotkey: self.hotkey.clone(),
            }]
        } else {
            self.profiles.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechTask {
    #[default]
    Transcribe,
    /// Translate speech into the configured `target_language` (requires a
    /// model whose translate task works, e.g. stock whisper-large-v3).
    Translate,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechBackend {
    /// Probe GPUs (discrete before integrated) and fall back to CPU.
    #[default]
    Auto,
    Cpu,
    Cuda,
    Vulkan,
    Metal,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechHotkeyMode {
    /// Ventrilo-style: record while the hotkey is held, transcribe on release.
    #[default]
    Hold,
    /// Press once to start recording, press again to stop and transcribe.
    Toggle,
}

pub(crate) fn validate_speech(speech: &SpeechConfig, shortcuts: &AppShortcuts) -> Result<()> {
    if !speech.enabled {
        return Ok(());
    }
    let mut bindings: Vec<(String, ShortcutBinding)> = Vec::new();
    for (index, profile) in speech.resolved_profiles().iter().enumerate() {
        let label = if profile.name.trim().is_empty() {
            // 1-based to match how profiles are labelled at runtime.
            format!("features.speech profile #{}", index + 1)
        } else {
            format!("features.speech profile `{}`", profile.name)
        };
        // An enabled profile with no model would fail only as a runtime log
        // line; reject it up front.
        if profile.model.trim().is_empty() {
            return Err(Error::Config(format!("{label} has no model path")));
        }
        if profile.hotkey.trim().is_empty() {
            // Only the first profile is reachable without a hotkey (it is the
            // mic-button default). A later hotkey-less profile can never be
            // selected, so reject it rather than silently ignoring it.
            if index > 0 {
                return Err(Error::Config(format!(
                    "{label} needs a push-to-talk hotkey (only the first profile can be mic-button only)"
                )));
            }
            continue;
        }
        let binding = ShortcutBinding::parse(&profile.hotkey)
            .map_err(|error| Error::Config(format!("{label} hotkey: {}", config_error_message(&error))))?;
        if let Some((other, _)) = bindings.iter().find(|(_, existing)| binding.overlaps(*existing)) {
            return Err(Error::Config(format!(
                "{label} hotkey `{}` overlaps {other}'s hotkey",
                profile.hotkey
            )));
        }
        validate_speech_binding(&label, &profile.hotkey, binding, shortcuts)?;
        bindings.push((label, binding));
    }
    Ok(())
}

/// `ShortcutBinding::parse` returns `Error::Config`, whose Display already
/// carries a `Config error:` prefix; strip it so nesting does not double it.
fn config_error_message(error: &Error) -> String {
    error
        .to_string()
        .strip_prefix("Config error: ")
        .map_or_else(|| error.to_string(), str::to_string)
}

/// Validate a single push-to-talk hotkey string against the shared shortcut
/// rules (parse, reserved Escape, bare-key, clipboard chords, and overlap
/// with global shortcuts) — WITHOUT the profile/model completeness checks,
/// so the settings binder can accept a chord before other fields are filled.
///
/// # Errors
/// Returns an error describing the first rule the hotkey violates.
pub fn validate_speech_hotkey(hotkey: &str, shortcuts: &AppShortcuts) -> Result<()> {
    let binding = ShortcutBinding::parse(hotkey)
        .map_err(|error| Error::Config(format!("hotkey: {}", config_error_message(&error))))?;
    validate_speech_binding("hotkey", hotkey, binding, shortcuts)
}

fn validate_speech_binding(
    label: &str,
    hotkey: &str,
    binding: ShortcutBinding,
    shortcuts: &AppShortcuts,
) -> Result<()> {
    // Escape is reserved: the frame handler treats every Escape press as
    // "cancel dictation", so an Escape binding would start and immediately
    // cancel a recording.
    if binding.key == ShortcutKey::Escape {
        return Err(Error::Config(format!(
            "{label} hotkey `{hotkey}` cannot use Escape (reserved for cancelling dictation)"
        )));
    }
    // A bare typing or navigation key as a hold hotkey would hijack that key
    // in every terminal. Require a modifier, or a function key.
    if binding.modifiers == crate::shortcuts::ShortcutModifiers::NONE
        && matches!(
            binding.key,
            ShortcutKey::Letter(_)
                | ShortcutKey::Digit(_)
                | ShortcutKey::Enter
                | ShortcutKey::Tab
                | ShortcutKey::Comma
                | ShortcutKey::Minus
                | ShortcutKey::Plus
                // Arrows drive cursor movement and shell history in every
                // terminal panel, so a bare arrow hotkey would hijack them.
                | ShortcutKey::ArrowDown
                | ShortcutKey::ArrowLeft
                | ShortcutKey::ArrowRight
                | ShortcutKey::ArrowUp
        )
    {
        return Err(Error::Config(format!(
            "{label} hotkey `{hotkey}` needs a modifier (e.g. Ctrl+{hotkey}); a bare key would hijack terminal input"
        )));
    }
    // egui-winit translates the primary modifier + C/X/V into synthetic
    // Copy/Cut/Paste events instead of key presses, so such a chord never
    // fires. This applies to both the platform-primary (`command`) and the
    // physical-`Control` spelling — on Linux/Windows they are the same key.
    // egui-winit's is_copy/cut/paste_command tests ONLY `modifiers.command`
    // and the key — shift and alt do not exempt a chord — and it pushes the
    // synthetic event instead of the key press. Mirror that predicate exactly
    // (the physical-Control spelling is the same key on Linux/Windows).
    if (binding.modifiers.command() || binding.modifiers.ctrl())
        && matches!(binding.key, ShortcutKey::Letter('C' | 'X' | 'V'))
    {
        return Err(Error::Config(format!(
            "{label} hotkey `{hotkey}` is reserved for clipboard operations and never reaches shortcut handling"
        )));
    }
    // The push-to-talk chord is consumed before other handlers; a binding
    // that overlaps a global shortcut would trigger both actions.
    let global_bindings = [
        ("command_palette", shortcuts.command_palette),
        ("new_terminal", shortcuts.new_terminal),
        ("focus_active_workspace", shortcuts.focus_active_workspace),
        ("fit_active_workspace", shortcuts.fit_active_workspace),
        ("open_remote_hosts", shortcuts.open_remote_hosts),
        ("toggle_sessions", shortcuts.toggle_sessions),
        ("toggle_sidebar", shortcuts.toggle_sidebar),
        ("toggle_hud", shortcuts.toggle_hud),
        ("toggle_minimap", shortcuts.toggle_minimap),
        ("align_workspaces_horizontally", shortcuts.align_workspaces_horizontally),
        ("toggle_settings", shortcuts.toggle_settings),
        ("zoom_reset", shortcuts.zoom_reset),
        ("zoom_in", shortcuts.zoom_in),
        ("zoom_out", shortcuts.zoom_out),
        ("fullscreen_panel", shortcuts.fullscreen_panel),
        ("exit_fullscreen_panel", shortcuts.exit_fullscreen_panel),
        ("fullscreen_window", shortcuts.fullscreen_window),
        ("save_editor", shortcuts.save_editor),
        ("search", shortcuts.search),
    ];
    if let Some((name, _)) = global_bindings.iter().find(|(_, global)| binding.overlaps(*global)) {
        return Err(Error::Config(format!(
            "{label} hotkey `{hotkey}` overlaps the `{name}` shortcut; choose an unused key"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Config, FeaturesConfig};

    #[test]
    fn speech_config_defaults_are_disabled_hold_f9() {
        let speech = FeaturesConfig::default().speech;
        assert!(!speech.enabled);
        assert_eq!(speech.language, "auto");
        assert_eq!(speech.task, super::SpeechTask::Transcribe);
        assert_eq!(speech.backend, super::SpeechBackend::Auto);
        assert_eq!(speech.hotkey, "F9");
        assert_eq!(speech.hotkey_mode, super::SpeechHotkeyMode::Hold);
    }
    #[test]
    fn speech_config_parses_from_yaml_and_roundtrips() {
        let yaml = r"
features:
  speech:
    enabled: true
    model: /models/nb-whisper-large-Q8_0.gguf
    language: no
    task: translate
    backend: cuda
    hotkey: F9
    hotkey_mode: toggle
";
        let config = Config::from_yaml(yaml).expect("speech config should parse");
        let speech = &config.features.speech;
        assert!(speech.enabled);
        assert_eq!(speech.model, "/models/nb-whisper-large-Q8_0.gguf");
        assert_eq!(speech.language, "no");
        assert_eq!(speech.task, super::SpeechTask::Translate);
        assert_eq!(speech.backend, super::SpeechBackend::Cuda);
        assert_eq!(speech.hotkey_mode, super::SpeechHotkeyMode::Toggle);

        let round_tripped = Config::from_yaml(&config.to_yaml().expect("serialize")).expect("re-parse");
        assert_eq!(round_tripped.features.speech, *speech);
    }
    #[test]
    fn config_without_speech_block_still_parses() {
        let config = Config::from_yaml("features:\n  attention_feed: false\n").expect("parse");
        assert!(!config.features.speech.enabled);
    }
    #[test]
    fn validate_rejects_malformed_speech_hotkey_only_when_enabled() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.model = "/m.gguf".to_string();
        config.features.speech.hotkey = "Ctrl++".to_string();
        let error = config.validate().expect_err("malformed hotkey must be rejected");
        assert!(error.to_string().contains("features.speech profile"));

        config.features.speech.enabled = false;
        config.validate().expect("disabled speech skips hotkey validation");

        config.features.speech.enabled = true;
        config.features.speech.hotkey = "F9".to_string();
        config.validate().expect("valid hotkey passes");
    }
    #[test]
    fn validate_rejects_clipboard_pseudo_event_hotkeys() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.model = "/m.gguf".to_string();
        for chord in ["Ctrl+C", "Ctrl+X", "Ctrl+V"] {
            config.features.speech.hotkey = chord.to_string();
            let error = config.validate().expect_err("clipboard chord must be rejected");
            assert!(error.to_string().contains("clipboard"), "{chord}: {error}");
        }
        // egui checks only `command` + key, so shift/alt variants are ALSO
        // turned into clipboard events and must be rejected too.
        for chord in ["Ctrl+Shift+C", "Ctrl+Alt+C", "Ctrl+Shift+V"] {
            config.features.speech.hotkey = chord.to_string();
            let error = config.validate().expect_err("clipboard chord must be rejected");
            assert!(error.to_string().contains("clipboard"), "{chord}: {error}");
        }
        // A different letter is unaffected.
        config.features.speech.hotkey = "Ctrl+Alt+D".to_string();
        config.validate().expect("non-clipboard chord passes");
    }
    #[test]
    fn validate_rejects_overlapping_profile_hotkeys() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.profiles = vec![
            super::SpeechProfile {
                name: "Norsk".to_string(),
                model: "/no.gguf".to_string(),
                hotkey: "F1".to_string(),
                ..super::SpeechProfile::default()
            },
            super::SpeechProfile {
                name: "English".to_string(),
                model: "/en.gguf".to_string(),
                hotkey: "F1".to_string(),
                ..super::SpeechProfile::default()
            },
        ];
        let error = config
            .validate()
            .expect_err("duplicate profile hotkeys must be rejected");
        assert!(error.to_string().contains("overlaps"));

        config.features.speech.profiles[1].hotkey = "F2".to_string();
        config.validate().expect("distinct profile hotkeys pass");
    }
    #[test]
    fn resolved_profiles_synthesizes_from_flat_fields() {
        let mut speech = super::SpeechConfig {
            model: "/models/a.gguf".to_string(),
            hotkey: "F9".to_string(),
            ..super::SpeechConfig::default()
        };
        let profiles = speech.resolved_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].model, "/models/a.gguf");
        assert_eq!(profiles[0].hotkey, "F9");

        speech.profiles.push(super::SpeechProfile {
            name: "English".to_string(),
            hotkey: "F2".to_string(),
            ..super::SpeechProfile::default()
        });
        assert_eq!(speech.resolved_profiles().len(), 1);
        assert_eq!(speech.resolved_profiles()[0].name, "English");
    }
    #[test]
    fn validate_rejects_bare_terminal_hotkeys() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.model = "/m.gguf".to_string();
        for bare in [
            "K",
            "5",
            "Enter",
            "Tab",
            "Comma",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            // The short aliases parse to the same keys and must be rejected too.
            "Left",
            "Right",
        ] {
            config.features.speech.hotkey = bare.to_string();
            let error = config.validate().expect_err("bare key must be rejected");
            assert!(error.to_string().contains("needs a modifier"), "{bare}: {error}");
        }
        config.features.speech.hotkey = "Ctrl+Alt+ArrowUp".to_string();
        config.validate().expect("modified arrow passes");

        // Bare function keys remain available as intentional global controls.
        config.features.speech.hotkey = "F9".to_string();
        config.validate().expect("bare function key passes");
    }

    #[test]
    fn validate_rejects_enabled_profile_without_model() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        let error = config.validate().expect_err("empty model must be rejected");
        assert!(error.to_string().contains("no model path"), "{error}");
    }

    #[test]
    fn validate_rejects_speech_hotkey_overlapping_global_shortcut() {
        let mut config = Config::default();
        config.features.speech.enabled = true;
        config.features.speech.model = "/m.gguf".to_string();
        config.features.speech.hotkey = "F11".to_string(); // fullscreen_panel default
        let error = config.validate().expect_err("overlapping hotkey must be rejected");
        assert!(error.to_string().contains("fullscreen_panel"));
    }
}
