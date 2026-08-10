use horizon_core::{Config, PanelKind, PanelResume, PresetConfig, SpeechProfile};

use super::detection::AgentProbeCache;
use super::{
    AvailabilitySummary, SAVE_OR_REVERT_MESSAGE, SpeechAgentSetupState, SpeechSetupAgent, SpeechSetupAgentAvailability,
    SpeechSetupLaunchGate, SpeechSetupProbeFailure, SpeechSetupReadiness, availability_summary, available_agents,
    selected_setup_preset, validate_setup_saved_config,
};

fn preset(name: &str, kind: PanelKind, command: Option<&str>, args: &[&str]) -> PresetConfig {
    PresetConfig {
        name: name.to_string(),
        alias: None,
        kind,
        command: command.map(str::to_string),
        args: args.iter().map(|argument| (*argument).to_string()).collect(),
        resume: PanelResume::Last,
        ssh_connection: None,
    }
}

fn saved_config_yaml() -> String {
    Config::default().to_yaml().expect("default config serializes")
}

#[test]
fn setup_saved_config_guard_rejects_a_missing_file() {
    let temp = tempfile::tempdir().expect("temporary config directory");
    let config_path = temp.path().join("config.yaml");

    let error =
        validate_setup_saved_config(&config_path, &saved_config_yaml()).expect_err("missing config must block setup");

    assert!(error.contains("is missing"), "unexpected guidance: {error}");
    assert!(error.contains("Save"), "unexpected guidance: {error}");
    assert!(error.contains("reopen Settings"), "unexpected guidance: {error}");
}

#[test]
fn setup_saved_config_guard_rejects_invalid_yaml() {
    let temp = tempfile::tempdir().expect("temporary config directory");
    let config_path = temp.path().join("config.yaml");
    std::fs::write(&config_path, "features: [").expect("write invalid config");

    let error =
        validate_setup_saved_config(&config_path, &saved_config_yaml()).expect_err("invalid config must block setup");

    assert!(error.contains("is invalid"), "unexpected guidance: {error}");
    assert!(error.contains("Save"), "unexpected guidance: {error}");
    assert!(error.contains("reopen Settings"), "unexpected guidance: {error}");
}

#[test]
fn setup_saved_config_guard_rejects_external_drift() {
    let temp = tempfile::tempdir().expect("temporary config directory");
    let config_path = temp.path().join("config.yaml");
    let expected = saved_config_yaml();
    std::fs::write(&config_path, format!("# external edit\n{expected}")).expect("write externally changed config");

    let error =
        validate_setup_saved_config(&config_path, &expected).expect_err("externally changed config must block setup");

    assert!(
        error.contains("changed outside Horizon"),
        "unexpected guidance: {error}"
    );
    assert!(error.contains("Save"), "unexpected guidance: {error}");
    assert!(error.contains("reopen Settings"), "unexpected guidance: {error}");
}

#[test]
fn setup_saved_config_guard_accepts_an_exact_valid_match() {
    let temp = tempfile::tempdir().expect("temporary config directory");
    let config_path = temp.path().join("config.yaml");
    let expected = saved_config_yaml();
    std::fs::write(&config_path, &expected).expect("write saved config");

    validate_setup_saved_config(&config_path, &expected).expect("exact valid config must allow setup");
}

#[test]
fn readiness_distinguishes_build_disabled_model_and_ready_states() {
    let mut speech = horizon_core::SpeechConfig::default();
    assert_eq!(
        SpeechSetupReadiness::classify(false, &speech),
        SpeechSetupReadiness::BuildMissing
    );
    assert_eq!(
        SpeechSetupReadiness::classify(true, &speech),
        SpeechSetupReadiness::Disabled
    );

    speech.enabled = true;
    assert_eq!(
        SpeechSetupReadiness::classify(true, &speech),
        SpeechSetupReadiness::ModelMissing
    );

    speech.model = "/models/starter.gguf".to_string();
    assert_eq!(
        SpeechSetupReadiness::classify(true, &speech),
        SpeechSetupReadiness::Ready
    );
}

#[test]
fn readiness_requires_every_explicit_profile_to_have_a_model() {
    let mut speech = horizon_core::SpeechConfig {
        enabled: true,
        profiles: vec![
            SpeechProfile {
                name: "Norwegian".to_string(),
                model: "/models/no.gguf".to_string(),
                ..SpeechProfile::default()
            },
            SpeechProfile {
                name: "English".to_string(),
                model: "   ".to_string(),
                ..SpeechProfile::default()
            },
        ],
        ..horizon_core::SpeechConfig::default()
    };

    assert_eq!(
        SpeechSetupReadiness::classify(true, &speech),
        SpeechSetupReadiness::ModelMissing
    );
    speech.profiles[1].model = "/models/en.gguf".to_string();
    assert_eq!(
        SpeechSetupReadiness::classify(true, &speech),
        SpeechSetupReadiness::Ready
    );
}

#[test]
fn launch_gate_requires_valid_saved_settings() {
    let saved = SpeechSetupLaunchGate::new(true, false);
    assert!(saved.can_launch());
    assert_eq!(saved.blocked_reason(), None);

    for blocked in [
        SpeechSetupLaunchGate::new(false, false),
        SpeechSetupLaunchGate::new(true, true),
        SpeechSetupLaunchGate::new(false, true),
    ] {
        assert!(!blocked.can_launch());
        assert_eq!(blocked.blocked_reason(), Some(SAVE_OR_REVERT_MESSAGE));
    }
}

#[test]
fn configured_agent_preset_wins_and_preserves_command_args() {
    let mut config = Config::default();
    config.presets.insert(
        0,
        preset(
            "Local Codex",
            PanelKind::Codex,
            Some("/opt/codex custom/bin/codex"),
            &["--no-alt-screen", "--profile", "speech"],
        ),
    );

    let selected = selected_setup_preset(&config, SpeechSetupAgent::Codex).expect("configured Codex preset");
    assert_eq!(selected.name, "Local Codex");
    assert_eq!(selected.command.as_deref(), Some("/opt/codex custom/bin/codex"));
    assert_eq!(selected.args, ["--no-alt-screen", "--profile", "speech"]);
}

#[test]
fn first_matching_configured_preset_is_the_deterministic_choice() {
    let config = Config {
        presets: vec![
            preset("First Claude", PanelKind::Claude, Some("/first/claude"), &["--first"]),
            preset(
                "Second Claude",
                PanelKind::Claude,
                Some("/second/claude"),
                &["--second"],
            ),
        ],
        ..Config::default()
    };

    let selected = selected_setup_preset(&config, SpeechSetupAgent::Claude).expect("Claude preset");
    assert_eq!(selected.name, "First Claude");
    assert_eq!(selected.command.as_deref(), Some("/first/claude"));
}

#[test]
fn missing_configured_agent_uses_horizon_safe_default() {
    let mut config = Config::default();
    config
        .presets
        .retain(|preset| !matches!(preset.kind, PanelKind::Codex | PanelKind::Claude));

    let codex = selected_setup_preset(&config, SpeechSetupAgent::Codex).expect("default Codex preset");
    assert_eq!(codex.kind, PanelKind::Codex);
    assert_eq!(codex.command, None);
    assert_eq!(codex.args, ["--no-alt-screen"]);
    assert_eq!(codex.resume, PanelResume::Fresh);

    let claude = selected_setup_preset(&config, SpeechSetupAgent::Claude).expect("default Claude preset");
    assert_eq!(claude.kind, PanelKind::Claude);
    assert_eq!(claude.command, None);
    assert_eq!(claude.args, ["--permission-mode", "auto"]);
    assert_eq!(claude.resume, PanelResume::Fresh);
}

fn probe_cache(codex: SpeechSetupAgentAvailability, claude: SpeechSetupAgentAvailability) -> AgentProbeCache {
    let mut probes = AgentProbeCache::new();
    probes.set_test_availability(codex, claude);
    probes
}

fn available(executable: &str) -> SpeechSetupAgentAvailability {
    SpeechSetupAgentAvailability::Available {
        executable: executable.to_string(),
    }
}

fn rendered_text(output: &egui::FullOutput) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(text_shape) => {
                text.push_str(text_shape.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
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

fn blocked_agent_actions_text(gate: SpeechSetupLaunchGate) -> String {
    let ctx = egui::Context::default();
    let mut state = SpeechAgentSetupState::new();
    state
        .probes
        .set_test_availability(available("/tools/codex"), SpeechSetupAgentAvailability::Missing);
    let config = Config::default();

    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(state.render_agent_actions(ui, &config, gate, false).is_none());
        });
    });
    rendered_text(&output)
}

#[test]
fn invalid_settings_keep_setup_action_visible_with_save_guidance() {
    let text = blocked_agent_actions_text(SpeechSetupLaunchGate::new(false, true));
    assert!(text.contains("Set up with Codex"), "rendered text: {text}");
    assert!(text.contains(SAVE_OR_REVERT_MESSAGE), "rendered text: {text}");
}

#[test]
fn unsaved_valid_settings_keep_setup_action_visible_with_save_guidance() {
    let text = blocked_agent_actions_text(SpeechSetupLaunchGate::new(true, true));
    assert!(text.contains("Set up with Codex"), "rendered text: {text}");
    assert!(text.contains(SAVE_OR_REVERT_MESSAGE), "rendered text: {text}");
}

#[test]
fn availability_exposes_both_one_and_no_agent_combinations() {
    let both = probe_cache(available("/tools/codex"), available("/tools/claude"));
    assert_eq!(
        available_agents(&both),
        [SpeechSetupAgent::Codex, SpeechSetupAgent::Claude]
    );
    assert_eq!(
        both.resolved_command(SpeechSetupAgent::Codex).as_deref(),
        Some("/tools/codex")
    );
    assert_eq!(availability_summary(&both), AvailabilitySummary::Available);

    let codex_only = probe_cache(available("/tools/codex"), SpeechSetupAgentAvailability::Missing);
    assert_eq!(available_agents(&codex_only), [SpeechSetupAgent::Codex]);
    assert_eq!(availability_summary(&codex_only), AvailabilitySummary::Available);

    let claude_only = probe_cache(SpeechSetupAgentAvailability::Missing, available("/tools/claude"));
    assert_eq!(available_agents(&claude_only), [SpeechSetupAgent::Claude]);

    let none = probe_cache(
        SpeechSetupAgentAvailability::Missing,
        SpeechSetupAgentAvailability::Missing,
    );
    assert!(available_agents(&none).is_empty());
    assert_eq!(availability_summary(&none), AvailabilitySummary::NoneFound);
}

#[test]
fn pending_and_failed_detection_are_never_reported_as_missing() {
    let pending = probe_cache(
        SpeechSetupAgentAvailability::Checking,
        SpeechSetupAgentAvailability::Missing,
    );
    assert_eq!(availability_summary(&pending), AvailabilitySummary::Checking);

    let unknown = probe_cache(
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Timeout),
        SpeechSetupAgentAvailability::Missing,
    );
    assert_eq!(availability_summary(&unknown), AvailabilitySummary::Unknown);
    assert_ne!(availability_summary(&unknown), AvailabilitySummary::NoneFound);
}

#[test]
fn setup_state_rescan_clears_launch_error_and_returns_to_checking() {
    let mut state = SpeechAgentSetupState::new();
    state.set_launch_error("spawn failed");
    state.probes.set_test_availability(
        SpeechSetupAgentAvailability::Missing,
        SpeechSetupAgentAvailability::Missing,
    );

    state.rescan();

    assert!(state.launch_error.is_none());
    assert_eq!(
        state.availability(SpeechSetupAgent::Codex),
        SpeechSetupAgentAvailability::Checking
    );
    assert_eq!(
        state.availability(SpeechSetupAgent::Claude),
        SpeechSetupAgentAvailability::Checking
    );
}

#[test]
fn setup_agent_metadata_matches_panel_contract() {
    assert_eq!(SpeechSetupAgent::Codex.panel_kind(), PanelKind::Codex);
    assert_eq!(SpeechSetupAgent::Codex.default_command(), "codex");
    assert_eq!(SpeechSetupAgent::Codex.panel_title(), "Speech Input Setup — Codex");
    assert_eq!(SpeechSetupAgent::Claude.panel_kind(), PanelKind::Claude);
    assert_eq!(SpeechSetupAgent::Claude.default_command(), "claude");
    assert_eq!(SpeechSetupAgent::Claude.panel_title(), "Speech Input Setup — Claude");
}
