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
            session_id: None,
            root: Some(PathBuf::from("/profiles/used-at-launch")),
            backend: None,
            hidden: false,
            remote_target: None,
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

#[test]
fn a_persisted_remote_target_restores_as_a_stopped_remote_panel() {
    let panel = PanelState {
        kind: PanelKind::Browser,
        browser_profile: Some(BrowserProfileState {
            session_id: None,
            root: None,
            backend: None,
            hidden: false,
            remote_target: Some("ios_phone".to_string()),
        }),
        ..PanelState::default()
    };
    let options = panel.to_panel_options(&crate::browser::BrowserConfig::default());
    assert_eq!(options.remote_target.as_deref(), Some("ios_phone"));
    assert!(
        options.remote_session.is_none(),
        "a restore never carries a session request: credentials are resolved per create"
    );
}

#[test]
fn shared_browser_identity_survives_save_restore_and_whole_session_copy_rekeys_it() {
    let panel = |id: &str| PanelState {
        local_id: id.into(),
        kind: PanelKind::Browser,
        browser_profile: Some(BrowserProfileState {
            session_id: Some("original".into()),
            ..BrowserProfileState::default()
        }),
        ..PanelState::default()
    };
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![panel("original"), panel("duplicate")],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let yaml = state.to_yaml().expect("serialize");
    let mut restored: RuntimeState = serde_yaml::from_str(&yaml).expect("restore");
    for panel in &restored.workspaces[0].panels {
        assert_eq!(
            panel
                .to_panel_options(&crate::browser::BrowserConfig::default())
                .browser_session_id
                .as_deref(),
            Some("original")
        );
    }
    restored.regenerate_browser_local_ids();
    let profiles: Vec<_> = restored.workspaces[0]
        .panels
        .iter()
        .map(|panel| panel.browser_profile.as_ref().expect("profile").session_id.as_deref())
        .collect();
    assert_eq!(profiles[0], profiles[1]);
    assert_eq!(profiles[0], Some(restored.workspaces[0].panels[0].local_id.as_str()));
    assert_ne!(profiles[0], Some("original"));
    assert_ne!(restored.workspaces[0].panels[0].local_id, "original");
}

#[test]
fn legacy_browser_profiles_restore_without_a_shared_identity() {
    let profile: BrowserProfileState = serde_yaml::from_str("root: /profiles\n").expect("legacy profile");
    assert!(profile.session_id.is_none());
}

#[test]
fn copying_a_standalone_browser_preserves_its_profile_owner_identity() {
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                local_id: "standalone".into(),
                kind: PanelKind::Browser,
                browser_profile: Some(BrowserProfileState {
                    session_id: Some("standalone".into()),
                    ..BrowserProfileState::default()
                }),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        focused_panel_local_id: Some("standalone".into()),
        ..RuntimeState::default()
    };
    state.regenerate_browser_local_ids();
    let panel = &state.workspaces[0].panels[0];
    assert_ne!(panel.local_id, "standalone");
    assert_eq!(
        panel.browser_profile.as_ref().expect("profile").session_id.as_deref(),
        Some(panel.local_id.as_str())
    );
    assert_eq!(state.focused_panel_local_id.as_deref(), Some(panel.local_id.as_str()));
}

#[test]
fn work_continuation_is_opt_in_and_independent_of_conversation_restore() {
    let legacy: PanelState = serde_yaml::from_str("kind: claude\nresume: last\n").expect("legacy panel");
    assert!(!legacy.work_resume.enabled);
    assert_eq!(legacy.work_resume.max_downtime_seconds, 3 * 60 * 60);
    assert_eq!(legacy.resume, PanelResume::Last);
    let mut panel = legacy;
    panel.work_resume.enabled = true;
    panel.work_resume.max_downtime_seconds = 60;
    let yaml = serde_yaml::to_string(&panel).expect("serialize");
    let restored: PanelState = serde_yaml::from_str(&yaml).expect("restore");
    assert_eq!(restored.work_resume, panel.work_resume);
    assert_eq!(restored.resume, PanelResume::Last);
    let options = restored.to_panel_options(&crate::browser::BrowserConfig::default());
    assert_eq!(options.work_resume, restored.work_resume);
}

#[test]
fn board_snapshot_preserves_the_panel_work_policy() {
    let mut board = crate::Board::new();
    let workspace = board.create_workspace("work");
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("panel");
    let panel = board.panel_mut(panel_id).expect("panel exists");
    panel.work_resume.enabled = true;
    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    assert!(state.workspaces[0].panels[0].work_resume.enabled);
}
