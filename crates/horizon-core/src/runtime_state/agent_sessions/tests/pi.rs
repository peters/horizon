use std::io::Cursor;

use super::super::{
    AgentSessionCatalog, AgentSessionRecord, PanelKind, PiSessionSummary, load_pi_sessions_from_dir,
    scan_pi_session_reader,
};
use super::stored_cwd;

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
    assert_eq!(session.cwd, stored_cwd("/repo"));
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

/// One line as `pi-subagents` writes a child run transcript: a record
/// envelope with `recordType`, the run identity, and the repeated cwd.
const PI_ARTIFACT_TRANSCRIPT: &str = concat!(
    r#"{"version":1,"recordType":"message","source":"async","runId":"8c9185cd","agent":"scout","childIndex":0,"cwd":"/repo","ts":1792835443579,"timestamp":"2026-10-24T09:50:43.579Z","sourceEventType":"initial_prompt","role":"user","text":"Scout the repository","message":{"role":"user","content":[{"type":"text","text":"Scout the repository"}]}}"#,
    "\n",
    r#"{"version":1,"recordType":"tool_start","source":"async","runId":"8c9185cd","agent":"scout","childIndex":0,"cwd":"/repo","ts":1792835444304,"timestamp":"2026-10-24T09:50:44.304Z","sourceEventType":"message_end","toolCallId":"call-1","toolName":"bash"}"#,
    "\n",
);

#[test]
fn parse_pi_session_skips_a_subagent_artifact_transcript() {
    assert!(
        parse_pi_session(Cursor::new(PI_ARTIFACT_TRANSCRIPT), "8c9185cd_scout_transcript", 7).is_none(),
        "a child run transcript is not a resumable Pi conversation"
    );
}

#[test]
fn load_pi_sessions_skips_subagent_artifact_transcripts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project = temp_dir.path().join("project");
    let artifacts = project.join("subagent-artifacts");
    std::fs::create_dir_all(&artifacts).expect("create artifact dir");
    std::fs::write(
        project.join("pi-session-123.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"pi-session-123\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Fix the build\"}}\n",
        ),
    )
    .expect("write pi session");
    std::fs::write(
        artifacts.join("8c9185cd_scout_transcript.jsonl"),
        PI_ARTIFACT_TRANSCRIPT,
    )
    .expect("write artifact transcript");
    std::fs::write(project.join("8c9185cd_scout.jsonl"), PI_ARTIFACT_TRANSCRIPT)
        .expect("write artifact stream outside the artifact dir");

    let sessions = load_pi_sessions_from_dir(temp_dir.path());
    let catalog = AgentSessionCatalog { sessions };
    let repo_sessions = catalog.recent_for(PanelKind::Pi, Some("/repo"));

    assert_eq!(repo_sessions.len(), 1, "only the conversation is a Pi session");
    assert_eq!(repo_sessions[0].session_id, "pi-session-123");
    assert_eq!(catalog.recent_for(PanelKind::Pi, None).len(), 1);
}

#[test]
fn load_pi_sessions_lists_project_files_and_filters_by_cwd() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let project = temp_dir.path().join("project");
    std::fs::create_dir_all(project.join("subagent-artifacts")).expect("create artifact dir");
    std::fs::write(
        project.join("pi-session-123.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"pi-session-123\",\"cwd\":\"/repo\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Fix the build\"}}\n",
        ),
    )
    .expect("write pi session");
    std::fs::create_dir_all(temp_dir.path().join("other-project")).expect("create other project dir");
    std::fs::write(
        temp_dir.path().join("other-project/pi-session-other.jsonl"),
        concat!(
            "{\"session_id\":\"pi-session-other\",\"metadata\":{\"cwd\":\"/other\"}}\n",
            "{\"role\":\"user\",\"content\":\"Other repo\"}\n",
        ),
    )
    .expect("write other pi session");
    std::fs::write(
        project.join("subagent-artifacts/run-1_scout_transcript.jsonl"),
        PI_ARTIFACT_TRANSCRIPT,
    )
    .expect("write artifact transcript");
    std::fs::create_dir_all(project.join("pi-session-123/8c9185cd/run-0")).expect("create child run dir");
    std::fs::write(
        project.join("pi-session-123/8c9185cd/run-0/session.jsonl"),
        concat!(
            "{\"type\":\"session\",\"id\":\"child-session\",\"cwd\":\"/repo\",\"parentSession\":\"pi-session-123.jsonl\"}\n",
            "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"Child run\"}}\n",
        ),
    )
    .expect("write child run session");

    let sessions = load_pi_sessions_from_dir(temp_dir.path());
    let catalog = AgentSessionCatalog { sessions };
    let repo_sessions = catalog.recent_for(PanelKind::Pi, Some("/repo"));

    assert_eq!(repo_sessions.len(), 1, "nested run files are not conversations");
    assert_eq!(repo_sessions[0].session_id, "pi-session-123");
    assert_eq!(repo_sessions[0].label.as_deref(), Some("Fix the build"));
    let other_sessions = catalog.recent_for(PanelKind::Pi, Some("/other"));
    assert_eq!(other_sessions.len(), 1);
    assert_eq!(other_sessions[0].session_id, "pi-session-other");
    assert_eq!(catalog.recent_for(PanelKind::Pi, None).len(), 2);
    assert!(catalog.recent_for(PanelKind::Pi, Some("/missing")).is_empty());
    assert!(catalog.recent_for(PanelKind::Claude, Some("/repo")).is_empty());
}
