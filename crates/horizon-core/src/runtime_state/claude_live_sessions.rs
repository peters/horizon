use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

const MAX_REGISTRY_ENTRIES: usize = 4096;
const MAX_REGISTRY_ENTRY_BYTES: u64 = 64 * 1024;

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

/// Deletion must verify every registry entry rather than silently skip failures.
pub(super) fn verified_live_claude_session_ids(home: &Path) -> crate::Result<HashSet<String>> {
    collect_verified_live_session_ids(&home.join(".claude/sessions"), process_is_alive)
}

#[derive(serde::Deserialize)]
struct VerifiedLiveSession {
    pid: u64,
    #[serde(rename = "sessionId")]
    session_id: String,
}

fn collect_verified_live_session_ids(
    dir: &Path,
    process_is_alive: impl Fn(u64) -> bool,
) -> crate::Result<HashSet<String>> {
    use crate::Error;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && std::fs::symlink_metadata(dir).is_err() => {
            return Ok(HashSet::new());
        }
        Err(error) => return Err(Error::State(format!("Cannot verify Claude live sessions: {error}"))),
    };
    let mut ids = HashSet::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_REGISTRY_ENTRIES {
            return Err(Error::State(
                "Cannot verify oversized Claude live-session registry".into(),
            ));
        }
        let entry = entry?;
        if entry.path().extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
            return Err(Error::State(
                "Cannot verify non-regular Claude live-session entry".into(),
            ));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(
                (rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW)
                    .bits()
                    .cast_signed(),
            );
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::State(
                "Cannot verify non-regular Claude live-session entry".into(),
            ));
        }
        let mut contents = String::new();
        file.take(MAX_REGISTRY_ENTRY_BYTES + 1).read_to_string(&mut contents)?;
        if contents.len() as u64 > MAX_REGISTRY_ENTRY_BYTES {
            return Err(Error::State("Cannot verify oversized Claude live-session entry".into()));
        }
        if !contents.trim_start().starts_with('{') {
            return Err(Error::State(
                "Cannot verify non-object Claude live-session entry".into(),
            ));
        }
        let record: VerifiedLiveSession = serde_json::from_str(&contents)
            .map_err(|error| Error::State(format!("Cannot verify Claude live-session entry: {error}")))?;
        let session_id = uuid::Uuid::parse_str(&record.session_id)
            .map_err(|_| Error::State("Cannot verify invalid Claude live-session ID".into()))?;
        if record.pid == 0 || path.file_stem().and_then(std::ffi::OsStr::to_str) != Some(&record.pid.to_string()) {
            return Err(Error::State(
                "Cannot verify incomplete Claude live-session entry".into(),
            ));
        }
        if process_is_alive(record.pid) {
            ids.insert(session_id.to_string());
        }
    }
    Ok(ids)
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

    use super::{claude_session_transcript_exists_in, collect_live_session_ids, collect_verified_live_session_ids};

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
    fn verified_registry_distinguishes_missing_from_unreadable() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            collect_verified_live_session_ids(&dir.path().join("missing"), |_| true)
                .expect("missing registry")
                .is_empty()
        );
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "not a directory").expect("blocked registry");
        assert!(collect_verified_live_session_ids(&blocked, |_| true).is_err());
        std::fs::create_dir(dir.path().join("101.json")).expect("unreadable entry");
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
    }

    #[test]
    fn verified_registry_rejects_malformed_and_incomplete_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        for contents in [
            "not json",
            r#"{"pid":101}"#,
            r#"{"sessionId":"live"}"#,
            r#"{"pid":0,"sessionId":"live"}"#,
            r#"{"pid":101,"sessionId":""}"#,
            r#"{"pid":101,"pid":102,"sessionId":"live"}"#,
            r#"{"pid":102,"pid":101,"sessionId":"live"}"#,
            r#"{"pid":101,"sessionId":"live","sessionId":"stale"}"#,
            r#"{"pid":101,"sessionId":null}"#,
            r#"{"pid":-1,"sessionId":"live"}"#,
            r#"{"pid":1.5,"sessionId":"live"}"#,
            r#"[101,"live"]"#,
        ] {
            write_entry(dir.path(), "101.json", contents);
            assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
        }
    }

    #[test]
    fn verified_registry_protects_live_sessions_and_ignores_stale_processes() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_entry(
            dir.path(),
            "101.json",
            r#"{"pid":101,"sessionId":"00000000-0000-0000-0000-000000000101"}"#,
        );
        write_entry(
            dir.path(),
            "102.json",
            r#"{"pid":102,"sessionId":"00000000-0000-0000-0000-000000000102"}"#,
        );
        write_entry(dir.path(), "ignored.txt", "not a registry entry");
        assert_eq!(
            collect_verified_live_session_ids(dir.path(), |pid| pid == 101).expect("verified registry"),
            HashSet::from(["00000000-0000-0000-0000-000000000101".to_string()])
        );
    }

    #[test]
    fn verified_registry_rejects_inconsistent_pids_and_normalizes_uuid_spelling() {
        let dir = tempfile::tempdir().expect("registry");
        for (name, pid, id) in [
            ("101.json", 102, "00000000-0000-0000-0000-000000000101"),
            ("not-a-pid.json", 101, "00000000-0000-0000-0000-000000000101"),
            ("101.json", 101, "not-a-uuid"),
        ] {
            write_entry(
                dir.path(),
                name,
                &serde_json::json!({"pid":pid,"sessionId":id}).to_string(),
            );
            assert!(collect_verified_live_session_ids(dir.path(), |_| false).is_err());
            std::fs::remove_file(dir.path().join(name)).expect("remove invalid entry");
        }
        write_entry(
            dir.path(),
            "101.json",
            r#"{"pid":101,"sessionId":"0000000000000000000000000000ABCD"}"#,
        );
        assert_eq!(
            collect_verified_live_session_ids(dir.path(), |_| true).expect("normalized ID"),
            HashSet::from(["00000000-0000-0000-0000-00000000abcd".to_string()])
        );
    }

    #[test]
    fn verified_registry_bounds_entry_bytes_and_total_directory_entries() {
        let dir = tempfile::tempdir().expect("registry");
        let record = r#"{"pid":101,"sessionId":"00000000-0000-0000-0000-000000000101"}"#;
        let contents = format!(
            "{record}{}",
            " ".repeat(usize::try_from(super::MAX_REGISTRY_ENTRY_BYTES).expect("entry limit fits") - record.len())
        );
        write_entry(dir.path(), "101.json", &contents);
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_ok());
        write_entry(dir.path(), "101.json", &(contents + " "));
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
        std::fs::remove_file(dir.path().join("101.json")).expect("remove oversized entry");
        for index in 0..super::MAX_REGISTRY_ENTRIES {
            write_entry(dir.path(), &format!("{index}.txt"), "ignored");
        }
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_ok());
        write_entry(dir.path(), "overflow.txt", "ignored");
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn verified_registry_rejects_links_and_fifos_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().expect("registry");
        let record = dir.path().join("record.txt");
        std::fs::write(
            &record,
            r#"{"pid":101,"sessionId":"00000000-0000-0000-0000-000000000101"}"#,
        )
        .expect("record");
        let entry = dir.path().join("101.json");
        std::os::unix::fs::symlink(&record, &entry).expect("linked entry");
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
        std::fs::remove_file(&entry).expect("remove link");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&entry)
                .status()
                .expect("create FIFO entry")
                .success()
        );
        assert!(collect_verified_live_session_ids(dir.path(), |_| true).is_err());
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
