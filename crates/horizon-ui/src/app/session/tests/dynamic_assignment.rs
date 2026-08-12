use super::*;

#[test]
fn collect_dynamic_binding_updates_assigns_scoped_groups_first() {
    let panels = vec![
        DynamicPanelBindingState {
            panel_id: PanelId(7),
            kind: PanelKind::OpenCode,
            cwd: String::new(),
            launched_at_millis: 10,
            session_binding: None,
            recent_output: false,
        },
        DynamicPanelBindingState {
            panel_id: PanelId(8),
            kind: PanelKind::OpenCode,
            cwd: "/repo".to_string(),
            launched_at_millis: 10,
            session_binding: None,
            recent_output: false,
        },
    ];
    let session = |session_id: &str, cwd: &str, updated_at| horizon_core::AgentSessionRecord {
        kind: PanelKind::OpenCode,
        session_id: session_id.to_string(),
        cwd: Some(cwd.to_string()),
        label: None,
        updated_at,
        interactive: true,
    };

    let updates = collect_dynamic_binding_updates(&panels, &HashSet::new(), |kind, cwd| {
        assert_eq!(kind, PanelKind::OpenCode);
        if cwd == Some("/repo") {
            vec![session("repo-session", "/repo", 20)]
        } else {
            vec![
                session("repo-session", "/repo", 20),
                session("other-session", "/other", 19),
            ]
        }
    });

    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].0, PanelId(8));
    assert_eq!(updates[0].1.session_id, "repo-session");
    assert_eq!(updates[1].0, PanelId(7));
    assert_eq!(updates[1].1.session_id, "other-session");
}
