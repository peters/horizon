use std::io::Cursor;

use rusqlite::Connection;
use uuid::Uuid;

use super::super::{
    AgentSessionCatalog, AgentSessionRecord, ClaudeSessionSummary, PanelKind, PiSessionSummary,
    load_claude_project_session_summary, load_opencode_sessions_from_path, load_pi_sessions_from_dir,
    scan_claude_session_reader, scan_pi_session_reader,
};

fn parse_claude_project_session<R: std::io::BufRead>(
    reader: R,
    fallback_session_id: &str,
    fallback_updated_at: i64,
) -> Option<AgentSessionRecord> {
    let mut summary = ClaudeSessionSummary::default();
    scan_claude_session_reader(reader, None, &mut summary);
    summary.into_record(fallback_session_id, fallback_updated_at)
}

fn parse_pi_session<R: std::io::BufRead>(
    reader: R,
    fallback_session_id: &str,
    fallback_updated_at: i64,
) -> Option<AgentSessionRecord> {
    let mut summary = PiSessionSummary::default();
    scan_pi_session_reader(reader, None, &mut summary);
    summary.into_record(fallback_session_id, fallback_updated_at)
}

#[test]
fn parse_claude_project_session_uses_resumable_jsonl_session_id() {
    let jsonl = concat!(
        "{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n",
        "{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n",
    );

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

    assert_eq!(session.kind, PanelKind::Claude);
    assert_eq!(session.session_id, "session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("reply with ok only"));
    assert_eq!(session.updated_at, 42);
}

#[test]
fn parse_claude_project_session_falls_back_to_filename_id() {
    let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert_eq!(session.session_id, "fallback-id");
    assert_eq!(session.cwd, None);
    assert_eq!(session.label.as_deref(), Some("Claude session"));
    assert_eq!(session.updated_at, 7);
}

#[test]
fn parse_claude_project_session_keeps_parent_uuid_without_sidechain_interactive() {
    let jsonl = "{\"type\":\"assistant\",\"sessionId\":\"session-123\",\"parentUuid\":\"message-1\"}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn parse_claude_project_session_keeps_parent_uuid_with_false_sidechain_interactive() {
    let jsonl = concat!(
        "{\"type\":\"assistant\",\"sessionId\":\"session-123\",",
        "\"isSidechain\":false,\"parentUuid\":\"message-1\"}\n",
    );

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn parse_claude_project_session_marks_sidechains_noninteractive() {
    let jsonl = "{\"type\":\"user\",\"sessionId\":\"child\",\"isSidechain\":true,\"parentUuid\":\"root\"}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(!session.interactive);
}

#[test]
fn parse_claude_project_session_uses_only_the_header_sidechain_flag() {
    let jsonl = concat!(
        "{\"type\":\"user\",\"sessionId\":\"session-123\",\"isSidechain\":false}\n",
        "{\"type\":\"assistant\",\"sessionId\":\"session-123\",\"isSidechain\":true}\n",
    );

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn load_claude_project_session_summary_reads_head_and_tail_metadata() {
    let path = std::env::temp_dir().join(format!("horizon-claude-session-{}.jsonl", Uuid::new_v4()));
    let mut content =
        String::from("{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n");
    for _ in 0..80 {
        content.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n");
    }
    content
        .push_str("{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n");
    std::fs::write(&path, content).expect("write temp session file");

    let session = load_claude_project_session_summary(&path, 9)
        .expect("load")
        .expect("session");
    std::fs::remove_file(&path).ok();

    assert_eq!(session.kind, PanelKind::Claude);
    assert_eq!(session.session_id, "session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("reply with ok only"));
    assert_eq!(session.updated_at, 9);
}

#[test]
fn load_opencode_sessions_reads_root_sessions_from_sqlite() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir.path().join("opencode.db");
    let conn = Connection::open(&sqlite_path).expect("sqlite");
    conn.execute_batch(
        "\
CREATE TABLE session (
id TEXT PRIMARY KEY,
title TEXT NOT NULL,
directory TEXT NOT NULL,
parent_id TEXT,
time_updated INTEGER NOT NULL,
time_archived INTEGER
);
INSERT INTO session (id, title, directory, parent_id, time_updated, time_archived) VALUES
('session-root', 'Fix auth flow', '/repo', NULL, 1000, NULL),
('session-child', 'Child', '/repo', 'session-root', 2000, NULL),
('session-archived', 'Archived', '/repo', NULL, 3000, 1),
('session-other', 'Other repo', '/other', NULL, 4000, NULL);
",
    )
    .expect("seed");

    let sessions = load_opencode_sessions_from_path(&sqlite_path).expect("opencode sessions");

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].kind, PanelKind::OpenCode);
    assert_eq!(sessions[0].session_id, "session-other");
    assert_eq!(sessions[0].cwd.as_deref(), Some("/other"));
    assert_eq!(sessions[1].session_id, "session-root");
    assert_eq!(sessions[1].cwd.as_deref(), Some("/repo"));
}

#[test]
fn parse_pi_session_uses_header_metadata_and_latest_user_message() {
    let jsonl = concat!(
        "{\"type\":\"session\",\"id\":\"pi-session-123\",\"cwd\":\"/repo\"}\n",
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"first prompt\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"working\"}}\n",
        "{\"type\":\"user_message\",\"text\":\"latest prompt\"}\n",
    );

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

    assert_eq!(session.kind, PanelKind::Pi);
    assert_eq!(session.session_id, "pi-session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("latest prompt"));
    assert_eq!(session.updated_at, 42);
}

#[test]
fn parse_pi_session_falls_back_to_filename_id_and_default_label() {
    let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"ok\"}}\n";

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert_eq!(session.kind, PanelKind::Pi);
    assert_eq!(session.session_id, "fallback-id");
    assert_eq!(session.cwd, None);
    assert_eq!(session.label.as_deref(), Some("Pi session"));
    assert_eq!(session.updated_at, 7);
}

#[test]
fn parse_pi_session_marks_parent_sessions_noninteractive() {
    let jsonl = "{\"type\":\"session\",\"id\":\"child\",\"parentSession\":\"/sessions/root.jsonl\"}\n";

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(!session.interactive);
}

#[test]
fn parse_pi_session_keeps_message_tree_parent_ids_interactive() {
    let jsonl = concat!(
        "{\"type\":\"session\",\"id\":\"root\",\"cwd\":\"/repo\"}\n",
        "{\"type\":\"message\",\"id\":\"entry-1\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"first\"}}\n",
        "{\"type\":\"message\",\"id\":\"entry-2\",\"parentId\":\"entry-1\",\"message\":{\"role\":\"assistant\",\"content\":\"done\"}}\n",
    );

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn load_pi_sessions_recurses_and_filters_by_cwd() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let nested = temp_dir.path().join("project/subdir");
    std::fs::create_dir_all(&nested).expect("create nested session dir");
    std::fs::write(
        nested.join("pi-session-123.jsonl"),
        concat!(
            "{\"session_id\":\"pi-session-123\",\"metadata\":{\"cwd\":\"/repo\"}}\n",
            "{\"role\":\"user\",\"content\":\"Fix the build\"}\n",
        ),
    )
    .expect("write pi session");
    std::fs::write(
        temp_dir.path().join("pi-session-other.jsonl"),
        concat!(
            "{\"session_id\":\"pi-session-other\",\"cwd\":\"/other\"}\n",
            "{\"role\":\"user\",\"content\":\"Other repo\"}\n",
        ),
    )
    .expect("write other pi session");

    let sessions = load_pi_sessions_from_dir(temp_dir.path()).expect("pi sessions");
    let catalog = AgentSessionCatalog { sessions };
    let repo_sessions = catalog.recent_for(PanelKind::Pi, Some("/repo"));

    assert_eq!(repo_sessions.len(), 1);
    assert_eq!(repo_sessions[0].session_id, "pi-session-123");
    assert_eq!(repo_sessions[0].label.as_deref(), Some("Fix the build"));
    assert!(catalog.recent_for(PanelKind::Pi, Some("/missing")).is_empty());
    assert!(catalog.recent_for(PanelKind::Claude, Some("/repo")).is_empty());
}
