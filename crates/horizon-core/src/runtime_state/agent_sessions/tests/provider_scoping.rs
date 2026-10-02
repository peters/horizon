use super::*;

#[test]
fn bootstrap_allows_equal_session_ids_from_different_providers() {
    let shared_id = "shared-session";
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                PanelState {
                    local_id: "codex".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    cwd: stored_cwd("/repo"),
                    resume: PanelResume::Last,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        shared_id.to_string(),
                        stored_cwd("/repo"),
                        None,
                        None,
                    )),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "claude".to_string(),
                    name: "Claude".to_string(),
                    kind: PanelKind::Claude,
                    cwd: stored_cwd("/repo"),
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let catalog = bootstrap_catalog(
        vec![AgentSessionRecord {
            kind: PanelKind::Claude,
            session_id: shared_id.to_string(),
            cwd: stored_cwd("/repo"),
            label: None,
            updated_at: 1,
            interactive: true,
        }],
        [(
            (PanelKind::Codex, shared_id.to_string()),
            ExactSessionResolution::Verified,
        )],
    );

    state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

    assert_eq!(state.workspaces[0].panels[0].stored_session_id(), Some(shared_id));
    assert_eq!(state.workspaces[0].panels[1].stored_session_id(), Some(shared_id));
}

#[test]
fn bootstrap_deduplicates_equal_session_ids_within_a_provider() {
    let duplicate_id = "duplicate-session";
    let bound_panel = |local_id: &str| PanelState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        kind: PanelKind::OpenCode,
        resume: PanelResume::Last,
        session_binding: Some(AgentSessionBinding::new(
            PanelKind::OpenCode,
            duplicate_id.to_string(),
            stored_cwd("/repo"),
            None,
            None,
        )),
        ..PanelState::default()
    };
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                bound_panel("first"),
                bound_panel("second"),
                PanelState {
                    local_id: "explicit".to_string(),
                    name: "explicit".to_string(),
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Session {
                        session_id: duplicate_id.to_string(),
                    },
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(state.bootstrap_missing_agent_bindings(&bootstrap_catalog(Vec::new(), []), &HashSet::new()));

    let panels = &state.workspaces[0].panels;
    assert_eq!(panels[0].stored_session_id(), Some(duplicate_id));
    assert_eq!(panels[1].stored_session_id(), None);
    assert!(matches!(panels[1].resume, PanelResume::Last));
    assert_eq!(panels[2].stored_session_id(), None);
    assert!(matches!(panels[2].resume, PanelResume::Fresh));
}

#[test]
fn provider_refresh_replaces_deleted_records_and_preserves_other_providers() {
    let record = |kind, id: &str, updated_at| AgentSessionRecord {
        kind,
        session_id: id.into(),
        cwd: None,
        label: None,
        updated_at,
        interactive: true,
    };
    let mut catalog = AgentSessionCatalog {
        sessions: vec![
            record(PanelKind::Claude, "removed", 10),
            record(PanelKind::Codex, "keep", 30),
        ],
    };
    catalog.replace_provider(
        PanelKind::Claude,
        AgentSessionCatalog {
            sessions: vec![
                record(PanelKind::Claude, "new", 20),
                record(PanelKind::Grok, "unrelated", 40),
            ],
        },
    );
    assert_eq!(catalog.recent_for(PanelKind::Codex, None)[0].session_id, "keep");
    assert_eq!(catalog.recent_for(PanelKind::Claude, None)[0].session_id, "new");
    assert!(catalog.recent_for(PanelKind::Grok, None).is_empty());
    catalog.replace_provider(PanelKind::Claude, AgentSessionCatalog::default());
    assert!(catalog.recent_for(PanelKind::Claude, None).is_empty());
    assert_eq!(catalog.recent_for(PanelKind::Codex, None).len(), 1);
}
