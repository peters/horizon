#[cfg(unix)]
use super::super::load_claude_sessions_from_dir;
use super::super::{AgentSessionRecord, load_claude_project_session_summary};
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
#[cfg(unix)] // Staging requires durable directory updates, currently supported only on Unix.
mod staging;

#[cfg(windows)]
#[test]
fn unsupported_directory_durability_preserves_windows_history() {
    let (_temp, projects, transcript, artifacts) = staged_fixture();
    assert!(!AgentSessionCatalog::supports_saved_session_deletion(PanelKind::Claude));
    let result = stage_claude_deletion(
        &projects,
        &transcript,
        Some(&artifacts),
        |_, _| panic!("must not rename"),
        |_| panic!("must not purge"),
    );
    assert!(result.err().expect("unsupported").to_string().contains("Unix"));
    assert_eq!(
        std::fs::read_to_string(transcript).expect("transcript"),
        "original transcript"
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("child"),
        "original child"
    );
}
