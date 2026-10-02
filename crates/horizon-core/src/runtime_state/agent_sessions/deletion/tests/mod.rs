use super::super::{AgentSessionRecord, load_claude_project_session_summary, load_claude_sessions_from_dir};
use super::*;

fn binding(id: u128) -> AgentSessionBinding {
    AgentSessionBinding::new(
        PanelKind::Claude,
        uuid::Uuid::from_u128(id).to_string(),
        Some("/example".into()),
        None,
        None,
    )
}

fn staged_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().expect("private store");
    let projects = temp.path().join("projects");
    let transcript = projects.join("project/conversation.jsonl");
    let artifacts = transcript.with_extension("");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    std::fs::write(&transcript, "original transcript").expect("transcript");
    std::fs::write(artifacts.join("agent.jsonl"), "original child").expect("child");
    (temp, projects, transcript, artifacts)
}

mod batch;
mod identity;
mod staging;
