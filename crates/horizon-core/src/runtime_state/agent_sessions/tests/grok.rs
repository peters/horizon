use super::super::grok as loader;
use super::*;

#[test]
fn provider_catalog_presence_includes_grok_panels() {
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                kind: PanelKind::Grok,
                resume: PanelResume::Session {
                    session_id: "grok-pinned".into(),
                },
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(AgentSessionCatalog::has_provider_panel(&runtime_state, PanelKind::Grok));
    assert!(!AgentSessionCatalog::has_provider_panel(
        &runtime_state,
        PanelKind::Claude
    ));
}

#[test]
fn load_grok_sessions_reads_sessions_from_search_index() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir.path().join("session_search.sqlite");
    let conn = Connection::open(&sqlite_path).expect("sqlite");
    conn.execute_batch(
        "\
CREATE TABLE session_docs (
session_id TEXT PRIMARY KEY,
cwd TEXT NOT NULL,
updated_at INTEGER NOT NULL,
title TEXT NOT NULL,
content TEXT NOT NULL,
content_hash TEXT NOT NULL
,
last_indexed_offset INTEGER NOT NULL DEFAULT 0
);
INSERT INTO session_docs (session_id, cwd, updated_at, title, content, content_hash) VALUES
('grok-older', '/repo', 1750000000, 'Older fix', 'payload', 'hash-1'),
('grok-newer', '/repo', 1750000000123, 'Newer fix', 'payload', 'hash-2'),
('grok-untitled', '/other', 1749999999, '', 'payload', 'hash-3');
",
    )
    .expect("seed");

    let sessions = loader::load_grok_sessions_from_path(&sqlite_path).expect("grok sessions");

    assert_eq!(sessions.len(), 3);
    assert_eq!(sessions[0].kind, PanelKind::Grok);
    assert_eq!(sessions[0].session_id, "grok-newer");
    assert_eq!(sessions[0].cwd.as_deref(), Some("/repo"));
    assert_eq!(sessions[0].label.as_deref(), Some("Newer fix"));
    assert_eq!(sessions[0].updated_at, 1_750_000_000_123);
    assert!(sessions[0].interactive);
    // Epoch-second timestamps are normalized to milliseconds so catalog
    // ordering and launch-window comparisons match the other providers.
    assert_eq!(sessions[1].session_id, "grok-older");
    assert_eq!(sessions[1].updated_at, 1_750_000_000_000);
    assert_eq!(sessions[2].session_id, "grok-untitled");
    assert_eq!(sessions[2].label.as_deref(), Some("Grok session"));
}

#[test]
fn load_grok_sessions_treats_missing_table_as_empty() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir.path().join("session_search.sqlite");
    let conn = Connection::open(&sqlite_path).expect("sqlite");
    conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .expect("seed");

    let sessions = loader::load_grok_sessions_from_path(&sqlite_path).expect("grok sessions");

    assert!(sessions.is_empty());
}
