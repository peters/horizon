use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Returns session ids currently owned by a running Claude Code process.
///
/// Claude Code maintains a live-session registry under `~/.claude/sessions/`,
/// one `<pid>.json` entry per running process, removed again on clean exit.
/// Sessions listed there are already open in some terminal, so automatically
/// resuming one of them would attach two UIs to the same conversation.
///
/// Entries whose process is no longer alive (stale files left by crashed
/// processes) are ignored on Linux. On other platforms every entry is treated
/// as live, which errs on the side of starting a fresh session.
#[must_use]
pub fn live_claude_session_ids() -> HashSet<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return HashSet::new();
    };
    collect_live_session_ids(&home.join(".claude/sessions"), process_is_alive)
}

fn collect_live_session_ids(dir: &Path, process_is_alive: impl Fn(u64) -> bool) -> HashSet<String> {
    let mut session_ids = HashSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return session_ids;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(session_id) = live_session_id(&contents, &process_is_alive) {
            session_ids.insert(session_id);
        }
    }
    session_ids
}

fn live_session_id(registry_entry: &str, process_is_alive: &impl Fn(u64) -> bool) -> Option<String> {
    let value: Value = serde_json::from_str(registry_entry).ok()?;
    let session_id = value.get("sessionId").and_then(Value::as_str)?;
    if session_id.is_empty() {
        return None;
    }
    let pid = value.get("pid").and_then(Value::as_u64)?;
    process_is_alive(pid).then(|| session_id.to_string())
}

#[cfg(target_os = "linux")]
fn process_is_alive(pid: u64) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(target_os = "linux"))]
fn process_is_alive(_pid: u64) -> bool {
    true
}

/// Returns true if a Claude Code transcript exists on disk for `session_id`.
///
/// Claude Code refuses `--resume` for session ids without an on-disk
/// transcript and `--session-id` for ids that already have one, so launch
/// commands must pick between the two based on what the store contains.
/// Transcripts live one level below `~/.claude/projects/`, named
/// `<session_id>.jsonl`; the project directory is scanned instead of derived
/// from the panel cwd because the cwd munging scheme is Claude Code's
/// implementation detail.
#[must_use]
pub fn claude_session_transcript_exists(session_id: &str) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    claude_session_transcript_exists_in(&home.join(".claude/projects"), session_id)
}

fn claude_session_transcript_exists_in(projects_dir: &Path, session_id: &str) -> bool {
    if session_id.is_empty() || session_id.contains(['/', '\\', '.']) {
        return false;
    }
    let file_name = format!("{session_id}.jsonl");
    let Ok(entries) = std::fs::read_dir(projects_dir) else {
        return false;
    };
    entries.flatten().any(|entry| entry.path().join(&file_name).is_file())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{claude_session_transcript_exists_in, collect_live_session_ids};

    fn write_entry(dir: &std::path::Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).expect("write registry entry");
    }

    #[test]
    fn collects_session_ids_for_live_processes_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_entry(
            dir.path(),
            "101.json",
            r#"{"pid":101,"sessionId":"session-live","cwd":"/repo","status":"idle"}"#,
        );
        write_entry(
            dir.path(),
            "102.json",
            r#"{"pid":102,"sessionId":"session-dead","cwd":"/repo","status":"idle"}"#,
        );

        let ids = collect_live_session_ids(dir.path(), |pid| pid == 101);

        assert_eq!(ids, HashSet::from(["session-live".to_string()]));
    }

    #[test]
    fn skips_malformed_and_incomplete_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_entry(dir.path(), "broken.json", "not json at all");
        write_entry(dir.path(), "no-session.json", r#"{"pid":103}"#);
        write_entry(dir.path(), "no-pid.json", r#"{"sessionId":"session-x"}"#);
        write_entry(dir.path(), "empty-session.json", r#"{"pid":104,"sessionId":""}"#);
        write_entry(dir.path(), "ignored.txt", r#"{"pid":105,"sessionId":"session-txt"}"#);

        let ids = collect_live_session_ids(dir.path(), |_| true);

        assert!(ids.is_empty());
    }

    #[test]
    fn missing_registry_dir_yields_no_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");

        let ids = collect_live_session_ids(&missing, |_| true);

        assert!(ids.is_empty());
    }

    #[test]
    fn transcript_lookup_finds_session_files_across_projects() {
        let projects = tempfile::tempdir().expect("tempdir");
        let project_dir = projects.path().join("-repo-one");
        std::fs::create_dir_all(&project_dir).expect("create project dir");
        std::fs::write(project_dir.join("session-1.jsonl"), "{}\n").expect("write transcript");

        assert!(claude_session_transcript_exists_in(projects.path(), "session-1"));
        assert!(!claude_session_transcript_exists_in(projects.path(), "session-2"));
        assert!(!claude_session_transcript_exists_in(projects.path(), ""));
        assert!(!claude_session_transcript_exists_in(projects.path(), "../session-1"));
        assert!(!claude_session_transcript_exists_in(
            &projects.path().join("missing"),
            "session-1"
        ));
    }
}
