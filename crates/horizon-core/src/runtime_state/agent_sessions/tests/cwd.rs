use super::super::{AgentSessionCatalog, AgentSessionRecord, PanelKind, normalize_cwd};

fn catalog_for(cwd: &str) -> AgentSessionCatalog {
    AgentSessionCatalog {
        sessions: vec![AgentSessionRecord {
            kind: PanelKind::Codex,
            session_id: "session-root".to_string(),
            cwd: normalize_cwd(Some(cwd)),
            label: Some("Saved session".to_string()),
            updated_at: 42,
            interactive: true,
        }],
    }
}

#[test]
fn recent_sessions_match_lexically_equivalent_cwds() {
    let parent = std::env::temp_dir().join("horizon-session-parent");
    let leaf = "horizon-session-cwd";
    let cwd = parent.join(leaf);
    let cwd = cwd.display().to_string();
    let parent = parent.display().to_string();
    let sep = std::path::MAIN_SEPARATOR;
    let catalog = catalog_for(&cwd);

    for query in [
        format!("{cwd}{sep}"),
        format!("{cwd}{sep}{sep}"),
        format!("{parent}{sep}{sep}{leaf}"),
        format!("{parent}{sep}.{sep}{leaf}"),
    ] {
        let sessions = catalog.recent_for(PanelKind::Codex, Some(&query));

        assert_eq!(sessions.len(), 1, "query {query} should match {cwd}");
        assert_eq!(sessions[0].session_id, "session-root");
    }
}
