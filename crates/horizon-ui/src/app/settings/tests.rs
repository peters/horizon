use egui::{Context, RawInput, Rect, Shape, Vec2};
use horizon_core::Config;

use super::{
    SettingsEditor, SettingsStatus, SettingsTab, deserialize_gui_config, load_settings_yaml,
    record_setup_saved_config_failure, render_gui_tab, speech,
};

fn rendered_text(output: &egui::FullOutput) -> String {
    fn collect(shape: &Shape, text: &mut String) {
        match shape {
            Shape::Text(text_shape) => {
                text.push_str(text_shape.galley.text());
                text.push('\n');
            }
            Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, text);
                }
            }
            _ => {}
        }
    }

    let mut text = String::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}

#[test]
fn invalid_yaml_still_renders_speech_setup_from_last_valid_config() {
    let last_valid = Config::default();
    let original = last_valid.to_yaml().expect("default config serializes");
    let invalid_buffer = "features:\n  speech:\n    enabled: [".to_string();
    let mut editor = SettingsEditor {
        buffer: invalid_buffer.clone(),
        original,
        status: SettingsStatus::Error("invalid YAML".to_string()),
        has_valid_saved_config: true,
        active_tab: SettingsTab::General,
        editing_config: Some(last_valid),
        speech_agent_setup: speech::SpeechAgentSetupState::new(),
    };
    let mut model_info_cache = speech::SpeechModelInfoCache::new();
    let ctx = Context::default();

    let output = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 900.0))),
            ..RawInput::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(
                    render_gui_tab(
                        ui,
                        SettingsTab::General,
                        &mut editor,
                        &mut model_info_cache,
                        speech::SpeechSetupLaunchGate::new(false, true),
                        None,
                        Vec2::new(700.0, 800.0),
                    )
                    .is_none()
                );
            });
        },
    );

    let text = rendered_text(&output);
    assert!(
        text.contains("Unable to parse current configuration"),
        "rendered text: {text}"
    );
    assert!(text.contains("Set up Speech Input"), "rendered text: {text}");
    assert_eq!(
        editor.buffer, invalid_buffer,
        "rendering must preserve the invalid YAML draft"
    );
}

#[test]
fn enabling_speech_without_a_model_keeps_manual_controls_editable_next_frame() {
    let mut config = Config::default();
    config.features.speech.enabled = true;
    let buffer = config.to_yaml().expect("config serializes before semantic validation");
    assert!(
        Config::from_yaml(&buffer).is_err(),
        "speech without a model is not yet saveable"
    );
    assert!(
        deserialize_gui_config(&buffer).is_ok(),
        "a semantic validation error must remain GUI-renderable"
    );

    let original = Config::default().to_yaml().expect("default config serializes");
    let mut speech_agent_setup = speech::SpeechAgentSetupState::new();
    speech_agent_setup.expand_manual_for_test();
    let mut editor = SettingsEditor {
        buffer: buffer.clone(),
        original,
        status: SettingsStatus::Error("speech model is required".to_string()),
        has_valid_saved_config: true,
        active_tab: SettingsTab::General,
        editing_config: Some(config),
        speech_agent_setup,
    };
    let mut model_info_cache = speech::SpeechModelInfoCache::new();
    let ctx = Context::default();

    let output = ctx.run(
        RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(900.0, 1_200.0))),
            ..RawInput::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(
                    render_gui_tab(
                        ui,
                        SettingsTab::General,
                        &mut editor,
                        &mut model_info_cache,
                        speech::SpeechSetupLaunchGate::new(false, true),
                        None,
                        Vec2::new(700.0, 1_100.0),
                    )
                    .is_none()
                );
            });
        },
    );

    let text = rendered_text(&output);
    assert!(text.contains("Speech Input"), "rendered text: {text}");
    assert!(text.contains("Model"), "manual controls disappeared: {text}");
    assert!(
        editor
            .editing_config
            .as_ref()
            .is_some_and(|config| config.features.speech.enabled),
        "the in-progress GUI state must remain editable"
    );
    assert_eq!(
        editor.buffer, buffer,
        "rendering must preserve the in-progress GUI draft"
    );
}

#[test]
fn missing_or_invalid_saved_config_keeps_setup_launch_blocked_until_save() {
    let temp = tempfile::tempdir().expect("temporary config directory");
    let config_path = temp.path().join("config.yaml");
    let fallback = Config::default().to_yaml().expect("default config serializes");

    let missing = load_settings_yaml(&config_path, fallback.clone());
    assert_eq!(missing.content, fallback);
    assert!(!missing.has_valid_saved_config);
    assert!(!speech::SpeechSetupLaunchGate::new(missing.has_valid_saved_config, false).can_launch());

    std::fs::write(&config_path, "features: [").expect("write invalid config");
    let invalid = load_settings_yaml(&config_path, fallback.clone());
    assert_eq!(invalid.content, fallback);
    assert!(!invalid.has_valid_saved_config);
    assert!(!speech::SpeechSetupLaunchGate::new(invalid.has_valid_saved_config, false).can_launch());

    std::fs::write(&config_path, &fallback).expect("write valid config");
    let valid = load_settings_yaml(&config_path, String::new());
    assert_eq!(valid.content, fallback);
    assert!(valid.has_valid_saved_config);
    assert!(speech::SpeechSetupLaunchGate::new(valid.has_valid_saved_config, false).can_launch());
}

#[test]
fn saved_config_drift_disables_repeated_setup_launches_until_save() {
    let config = Config::default();
    let yaml = config.to_yaml().expect("default config serializes");
    let mut editor = SettingsEditor {
        buffer: yaml.clone(),
        original: yaml,
        status: SettingsStatus::None,
        has_valid_saved_config: true,
        active_tab: SettingsTab::General,
        editing_config: Some(config),
        speech_agent_setup: speech::SpeechAgentSetupState::new(),
    };

    record_setup_saved_config_failure(&mut editor, "saved config changed".to_string());

    assert!(!editor.has_valid_saved_config);
    assert!(!speech::SpeechSetupLaunchGate::new(editor.has_valid_saved_config, false).can_launch());
}
