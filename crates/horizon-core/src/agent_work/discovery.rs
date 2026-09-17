use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    Absent,
    Stale,
    Live,
}

pub(super) fn session_presence(home: &Path, session: &str) -> io::Result<Presence> {
    let mut result = Presence::Absent;
    let entries = std::fs::read_dir(home.join(".claude/sessions"))?;
    for (index, entry) in entries.enumerate() {
        if index >= 4096 {
            return Err(io::Error::other("session registry exceeds limit"));
        }
        let path = entry?.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            continue;
        }
        let mut bytes = Vec::new();
        File::open(path)?.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::other("oversized session entry"));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let id = value
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| io::Error::other("missing session identity"))?;
        if id != session {
            continue;
        }
        let pid = value
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::other("missing process identity"))?;
        if !cfg!(target_os = "linux") {
            return Err(io::Error::other("process liveness is not verified on this platform"));
        }
        if Path::new("/proc").join(pid.to_string()).exists() {
            return Ok(Presence::Live);
        }
        result = Presence::Stale;
    }
    Ok(result)
}

pub(super) fn transcript(home: &Path, session: &str) -> Option<PathBuf> {
    if session.is_empty()
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let name = format!("{session}.jsonl");
    let mut found = None;
    for (index, entry) in std::fs::read_dir(home.join(".claude/projects")).ok()?.enumerate() {
        if index >= 4096 {
            return None;
        }
        let path = entry.ok()?.path().join(&name);
        if path.is_file() {
            if found.is_some() {
                return None;
            }
            found = Some(path);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_registry_is_uncertain_and_live_process_wins_over_stale() {
        let temp = tempfile::tempdir().expect("home");
        let dir = temp.path().join(".claude/sessions");
        assert!(session_presence(temp.path(), "session").is_err());
        std::fs::create_dir_all(&dir).expect("registry");
        assert_eq!(
            session_presence(temp.path(), "session").expect("empty"),
            Presence::Absent
        );
        std::fs::write(dir.join("stale.json"), r#"{"sessionId":"session","pid":4294967295}"#).expect("entry");
        if cfg!(target_os = "linux") {
            assert_eq!(
                session_presence(temp.path(), "session").expect("stale"),
                Presence::Stale
            );
        }
        std::fs::write(
            dir.join("live.json"),
            format!(r#"{{"sessionId":"session","pid":{}}}"#, std::process::id()),
        )
        .expect("entry");
        if cfg!(target_os = "linux") {
            assert_eq!(session_presence(temp.path(), "session").expect("live"), Presence::Live);
        } else {
            assert!(session_presence(temp.path(), "session").is_err());
        }
        std::fs::remove_file(dir.join("live.json")).expect("remove");
        std::fs::write(dir.join("bad.json"), "broken").expect("entry");
        assert!(session_presence(temp.path(), "session").is_err());
    }
}
