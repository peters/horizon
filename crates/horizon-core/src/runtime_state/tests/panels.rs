use super::*;

#[test]
fn from_board_records_each_browser_launch_profile_root() {
    let temp = tempfile::tempdir().expect("temporary profile root");
    let launched_root = temp.path().join("launched-profiles");
    let mut board = Board::new();
    let workspace = board.create_workspace("browser");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Browser,
                local_id: Some("saved-browser".to_string()),
                visible: false,
                browser_config: Some(crate::browser::BrowserConfig {
                    command: Some(temp.path().join("missing-chrome").display().to_string()),
                    profile_root: Some(launched_root.clone()),
                    ..crate::browser::BrowserConfig::default()
                }),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("Browser panel state should start");

    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let saved = &state.workspaces[0].panels[0];

    assert_eq!(saved.name_is_custom, Some(false));
    assert_eq!(
        saved.browser_profile.as_ref().and_then(|profile| profile.root.as_ref()),
        Some(&launched_root)
    );
    assert!(saved.browser_profile.as_ref().is_some_and(|profile| profile.hidden));
    assert!(
        !saved
            .to_panel_options(&crate::browser::BrowserConfig::default())
            .visible
    );
    board.shutdown_terminal_panels();
}

#[test]
fn empty_committed_browser_url_overrides_the_requested_command() {
    let panel = PanelState {
        kind: PanelKind::Browser,
        command: Some("https://requested.example".to_string()),
        browser_url: Some(String::new()),
        ..PanelState::default()
    };

    let options = panel.to_panel_options(&crate::browser::BrowserConfig::default());

    assert_eq!(options.command.as_deref(), Some(""));
}

#[test]
fn generated_panel_name_metadata_survives_yaml_roundtrip() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                name: "127.0.0.1".to_string(),
                name_is_custom: Some(false),
                kind: PanelKind::Browser,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let yaml = state.to_yaml().expect("serialize runtime state");
    let restored: RuntimeState = serde_yaml::from_str(&yaml).expect("deserialize runtime state");
    let options = restored.workspaces[0].panels[0].to_panel_options(&crate::browser::BrowserConfig::default());

    assert_eq!(options.name.as_deref(), Some("127.0.0.1"));
    assert_eq!(options.name_is_custom, Some(false));
}

#[test]
fn legacy_panel_name_metadata_keeps_supplied_name_inference() {
    let panel: PanelState = serde_yaml::from_str("name: Pinned name\n").expect("deserialize legacy panel state");
    let options = panel.to_panel_options(&crate::browser::BrowserConfig::default());

    assert_eq!(options.name.as_deref(), Some("Pinned name"));
    assert_eq!(options.name_is_custom, None);
}

#[test]
fn persisted_browser_profile_root_survives_other_config_changes() {
    let panel = PanelState {
        kind: PanelKind::Browser,
        browser_profile: Some(BrowserProfileState {
            root: Some(PathBuf::from("/profiles/used-at-launch")),
            backend: None,
            hidden: false,
        }),
        ..PanelState::default()
    };
    let current_config = crate::browser::BrowserConfig {
        quality: 73,
        profile_root: Some(PathBuf::from("/profiles/new-config")),
        ..crate::browser::BrowserConfig::default()
    };

    let restored = panel
        .to_panel_options(&current_config)
        .browser_config
        .expect("Browser restore config");

    assert_eq!(restored.profile_root, Some(PathBuf::from("/profiles/used-at-launch")));
    assert_eq!(restored.quality, 73);
}

#[test]
fn persisted_default_browser_profile_root_overrides_a_later_custom_root() {
    let panel = PanelState {
        kind: PanelKind::Browser,
        browser_profile: Some(BrowserProfileState::default()),
        ..PanelState::default()
    };
    let current_config = crate::browser::BrowserConfig {
        profile_root: Some(PathBuf::from("/profiles/new-config")),
        ..crate::browser::BrowserConfig::default()
    };

    let restored = panel
        .to_panel_options(&current_config)
        .browser_config
        .expect("Browser restore config");

    assert!(restored.profile_root.is_none());
}

#[test]
fn persisted_default_browser_profile_root_survives_yaml_roundtrip() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                kind: PanelKind::Browser,
                browser_profile: Some(BrowserProfileState::default()),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let yaml = state.to_yaml().expect("serialize runtime state");
    let restored: RuntimeState = serde_yaml::from_str(&yaml).expect("deserialize runtime state");

    assert!(restored.workspaces[0].panels[0].browser_profile.is_some());
    assert!(
        restored.workspaces[0].panels[0]
            .browser_profile
            .as_ref()
            .is_some_and(|profile| profile.root.is_none())
    );
}

#[test]
fn pi_panel_state_round_trips_through_runtime_yaml() {
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Pi".to_string(),
                kind: PanelKind::Pi,
                resume: PanelResume::Session {
                    session_id: "pi-session-123".to_string(),
                },
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Pi,
                    "pi-session-123".to_string(),
                    Some("/repo".to_string()),
                    Some("Fix the build".to_string()),
                    Some(42),
                )),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let yaml = state.to_yaml().expect("serialize runtime state");
    assert!(yaml.contains("kind: pi"));

    let reloaded: RuntimeState = serde_yaml::from_str(&yaml).expect("deserialize runtime state");
    let panel = &reloaded.workspaces[0].panels[0];
    assert_eq!(panel.kind, PanelKind::Pi);
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("pi-session-123")
    );
}
