use super::*;

fn home() -> (tempfile::TempDir, HorizonHome) {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = HorizonHome::from_root(dir.path().to_path_buf());
    (dir, home)
}

#[test]
fn defaults_to_claude_with_subscription() {
    let (_dir, home) = home();
    let settings = AssistantSettings::load(&home);
    assert_eq!(settings.agent, PanelKind::Claude);
    assert_eq!(settings.auth, AssistantAuth::Subscription);
}

#[test]
fn settings_round_trip() {
    let (_dir, home) = home();
    let settings = AssistantSettings {
        agent: PanelKind::Codex,
        auth: AssistantAuth::ApiKey,
        ..AssistantSettings::default()
    };
    settings.save(&home).expect("save");
    assert_eq!(AssistantSettings::load(&home), settings);
}

#[test]
fn unsupported_stored_agent_falls_back_to_default() {
    let (dir, home) = home();
    std::fs::create_dir_all(dir.path().join("assistant")).expect("dir");
    std::fs::write(
        dir.path().join("assistant/settings.json"),
        r#"{"agent":"shell","auth":"subscription"}"#,
    )
    .expect("write");
    assert_eq!(AssistantSettings::load(&home), AssistantSettings::default());
}

#[test]
fn subscription_needs_no_key_and_adds_no_environment() {
    let (_dir, home) = home();
    let settings = AssistantSettings::default();
    assert!(settings.launch_readiness(&home).is_ok());
    assert!(settings.launch_env(&home).expect("env").is_empty());
}

#[test]
fn api_key_mode_requires_a_saved_key() {
    let (_dir, home) = home();
    let settings = AssistantSettings {
        agent: PanelKind::Claude,
        auth: AssistantAuth::ApiKey,
        ..AssistantSettings::default()
    };
    assert!(settings.launch_readiness(&home).is_err());
    save_api_key(&home, PanelKind::Claude, "  sk-test  \n").expect("save key");
    assert!(settings.launch_readiness(&home).is_ok());
    let env = settings.launch_env(&home).expect("env");
    assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-test"));
}

#[test]
fn codex_key_uses_the_openai_variable() {
    let (_dir, home) = home();
    save_api_key(&home, PanelKind::Codex, "sk-codex").expect("save key");
    let settings = AssistantSettings {
        agent: PanelKind::Codex,
        auth: AssistantAuth::ApiKey,
        ..AssistantSettings::default()
    };
    let env = settings.launch_env(&home).expect("env");
    assert_eq!(env.get("OPENAI_API_KEY").map(String::as_str), Some("sk-codex"));
}

#[test]
fn agents_without_a_key_binding_reject_api_key_mode() {
    let (_dir, home) = home();
    let settings = AssistantSettings {
        agent: PanelKind::Gemini,
        auth: AssistantAuth::ApiKey,
        ..AssistantSettings::default()
    };
    assert!(settings.launch_readiness(&home).is_err());
    assert!(save_api_key(&home, PanelKind::Gemini, "key").is_err());
}

#[test]
fn rejects_blank_and_multiline_keys() {
    let (_dir, home) = home();
    assert!(save_api_key(&home, PanelKind::Claude, "   ").is_err());
    assert!(save_api_key(&home, PanelKind::Claude, "a\nb").is_err());
}

#[test]
fn removing_a_key_is_idempotent() {
    let (_dir, home) = home();
    save_api_key(&home, PanelKind::Claude, "sk-test").expect("save key");
    assert!(has_api_key(&home, PanelKind::Claude));
    remove_api_key(&home, PanelKind::Claude).expect("remove");
    remove_api_key(&home, PanelKind::Claude).expect("remove again");
    assert!(!has_api_key(&home, PanelKind::Claude));
}

#[cfg(unix)]
#[test]
fn stored_key_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, home) = home();
    save_api_key(&home, PanelKind::Claude, "sk-test").expect("save key");
    let mode = std::fs::metadata(dir.path().join("assistant/anthropic-api-key"))
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn asking_before_sending_is_the_default_and_is_not_part_of_the_engine() {
    let (dir, home) = home();
    assert!(AssistantSettings::default().ask_before_send);
    // Settings written before the preference existed keep the safe default.
    std::fs::create_dir_all(dir.path().join("assistant")).expect("dir");
    std::fs::write(
        dir.path().join("assistant/settings.json"),
        r#"{"agent":"claude","auth":"subscription"}"#,
    )
    .expect("write");
    assert!(AssistantSettings::load(&home).ask_before_send);

    let relaxed = AssistantSettings {
        ask_before_send: false,
        ..AssistantSettings::default()
    };
    assert!(relaxed.same_engine(&AssistantSettings::default()));
    let other = AssistantSettings {
        agent: PanelKind::Codex,
        ..AssistantSettings::default()
    };
    assert!(!other.same_engine(&AssistantSettings::default()));
}

#[test]
fn only_the_launch_token_matches_and_a_missing_one_never_does() {
    let token = launch_token().to_string();
    assert!(token_matches(Some(&token)));
    assert!(!token_matches(None));
    assert!(!token_matches(Some("")));
    assert!(!token_matches(Some("horizon-assistant")));
    assert!(!token_matches(Some(&token[..token.len() - 1])));
    assert!(!token_matches(Some(&format!("{token}x"))));
    assert_eq!(launch_token(), token, "the token is fixed for the process");
}
