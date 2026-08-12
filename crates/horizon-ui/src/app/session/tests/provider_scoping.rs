use super::*;

#[test]
fn collect_dynamic_binding_updates_scopes_reserved_ids_by_provider() {
    let panels = vec![DynamicPanelBindingState {
        panel_id: PanelId(7),
        kind: PanelKind::Codex,
        cwd: "/repo".to_string(),
        launched_at_millis: 10,
        session_binding: None,
        recent_output: false,
    }];
    let reserved = HashSet::from([horizon_core::AgentSessionKey::new(PanelKind::Claude, "shared-session")]);
    let updates = collect_dynamic_binding_updates(&panels, &reserved, |_, _| {
        vec![horizon_core::AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "shared-session".to_string(),
            cwd: Some("/repo".to_string()),
            label: None,
            updated_at: 12,
            interactive: true,
        }]
    });

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].1.session_id, "shared-session");
}

#[test]
fn rebind_allows_equal_session_ids_from_different_providers() {
    let (_temp, mut app) = test_app();
    let workspace_id = app.board.create_workspace("test");
    let (command, args) = exiting_command();
    let shared_id = "shared-session";
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Claude,
                command: Some(command.clone()),
                args: args.clone(),
                resume: PanelResume::Last,
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Claude,
                    shared_id.to_string(),
                    Some("/repo".to_string()),
                    None,
                    Some(42),
                )),
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("create Claude panel");
    let target_id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                command: Some(command),
                args,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("create Codex panel");
    let binding = AgentSessionBinding::new(
        PanelKind::Codex,
        shared_id.to_string(),
        Some("/repo".to_string()),
        None,
        Some(43),
    );

    assert!(app.rebind_and_restart_panel_session(target_id, binding));
    assert_eq!(app.panels_to_restart, vec![target_id]);
}
