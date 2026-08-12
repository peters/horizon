use super::*;

#[test]
fn failed_bootstrap_recovery_deduplicates_other_provider_bindings() {
    let (_temp, mut app) = test_app();
    let (command, args) = exiting_command();
    let duplicate_binding =
        || AgentSessionBinding::new(PanelKind::OpenCode, "duplicate-session".to_string(), None, None, None);
    app.pending_startup_runtime_state = Some(RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "alpha".to_string(),
            panels: vec![
                PanelState {
                    local_id: "codex".to_string(),
                    name: "Codex".to_string(),
                    kind: PanelKind::Codex,
                    command: Some(command.clone()),
                    args: args.clone(),
                    resume: PanelResume::Session {
                        session_id: "unverified-codex".to_string(),
                    },
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "first".to_string(),
                    name: "First OpenCode".to_string(),
                    kind: PanelKind::OpenCode,
                    command: Some(command.clone()),
                    args: args.clone(),
                    resume: PanelResume::Last,
                    session_binding: Some(duplicate_binding()),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "second".to_string(),
                    name: "Second OpenCode".to_string(),
                    kind: PanelKind::OpenCode,
                    command: Some(command),
                    args,
                    resume: PanelResume::Last,
                    session_binding: Some(duplicate_binding()),
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    });
    app.startup_bootstrap_failure = Some(StartupBootstrapFailure::WorkerDisconnected);

    app.handle_startup_bootstrap_failure(StartupBootstrapFailureAction::ContinueWithoutExactResumes);

    let opencode_panels: Vec<_> = app
        .board
        .panels
        .iter()
        .filter(|panel| panel.kind == PanelKind::OpenCode)
        .collect();
    assert_eq!(opencode_panels.len(), 2);
    assert_eq!(
        super::super::panel_session_id(opencode_panels[0]),
        Some("duplicate-session")
    );
    assert_eq!(super::super::panel_session_id(opencode_panels[1]), None);
    assert!(matches!(opencode_panels[1].resume, PanelResume::Last));
}
