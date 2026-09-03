use super::super::{AgentSessionCatalog, AgentSessionRecord, PanelKind, normalize_cwd};

#[test]
fn recent_sessions_match_cwd_with_a_trailing_separator() {
    let cwd = std::env::temp_dir().join("horizon-session-cwd");
    let cwd = cwd.display().to_string();
    let cwd_with_separator = format!("{cwd}{}", std::path::MAIN_SEPARATOR);
    let catalog = AgentSessionCatalog {
        sessions: vec![AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-root".to_string(),
            cwd: normalize_cwd(Some(&cwd)),
            label: Some("Saved session".to_string()),
            updated_at: 42,
            interactive: true,
        }],
    };

    let sessions = catalog.recent_for(PanelKind::Codex, Some(&cwd_with_separator));

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "session-root");
}
