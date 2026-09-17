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
        let process = Path::new("/proc").join(pid.to_string());
        if process.try_exists()? {
            if let Some(matches) = matches_process_instance(&process, &value)? {
                return if matches {
                    Ok(Presence::Live)
                } else {
                    Err(io::Error::other(
                        "session registry belongs to a different process instance",
                    ))
                };
            }
            let mut command = Vec::new();
            File::open(process.join("cmdline"))?
                .take(64 * 1024 + 1)
                .read_to_end(&mut command)?;
            if command.len() > 64 * 1024 || !matches_session_command(&command, session) {
                return Err(io::Error::other("live process session identity is uncertain"));
            }
            return Ok(Presence::Live);
        }
        result = Presence::Stale;
    }
    Ok(result)
}

fn matches_process_instance(process: &Path, entry: &serde_json::Value) -> io::Result<Option<bool>> {
    if entry.get("procStart").is_none() && entry.get("pidDomain").is_none() {
        return Ok(None);
    }
    let started = entry
        .get("procStart")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::other("missing process start identity"))?;
    let domain = entry
        .get("pidDomain")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::other("missing process domain identity"))?;
    let stat = std::fs::read_to_string(process.join("stat"))?;
    // The parenthesized process name may itself contain spaces or parentheses.
    let actual_start = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19));
    let machine = std::fs::read_to_string("/etc/machine-id")?;
    let namespace = std::fs::read_link("/proc/self/ns/pid")?;
    let actual_domain = format!("linux:{}:{}", machine.trim(), namespace.display());
    Ok(Some(actual_start == Some(started) && actual_domain == domain))
}

fn matches_session_command(command: &[u8], session: &str) -> bool {
    let Ok(command) = std::str::from_utf8(command) else {
        return false;
    };
    let arguments: Vec<_> = command.split_terminator('\0').collect();
    let Some(program) = arguments.first().and_then(|program| Path::new(program).file_name()) else {
        return false;
    };
    let provider = program == "claude"
        || (matches!(program.to_str(), Some("sh" | "bash" | "node" | "bun"))
            && arguments
                .get(1)
                .and_then(|script| Path::new(script).file_name())
                .is_some_and(|name| name == "claude"));
    provider
        && arguments
            .windows(2)
            .any(|pair| matches!(pair[0], "--resume" | "--session-id") && pair[1] == session)
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
    fn malformed_registry_and_reused_process_ids_are_uncertain() {
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
        assert!(session_presence(temp.path(), "session").is_err());
        std::fs::remove_file(dir.join("live.json")).expect("remove");
        std::fs::write(dir.join("bad.json"), "broken").expect("entry");
        assert!(session_presence(temp.path(), "session").is_err());
    }

    #[test]
    fn live_identity_requires_both_provider_and_exact_session_arguments() {
        assert!(matches_session_command(b"/bin/claude\0--resume\0session\0", "session"));
        assert!(matches_session_command(
            b"/bin/sh\0/tmp/claude\0--session-id\0session\0",
            "session"
        ));
        for command in [
            b"/bin/claude\0--resume\0other\0".as_slice(),
            b"/bin/claude\0--resume\0session-extra\0",
            b"/bin/unrelated\0--resume\0session\0",
            b"/bin/sh\0-c\0claude\0--resume\0session\0",
            b"/bin/claude\0session\0",
        ] {
            assert!(!matches_session_command(command, "session"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_exact_session_provider_process_is_live_even_with_a_stale_entry() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct DisposableChild(Child);
        impl Drop for DisposableChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let temp = tempfile::tempdir().expect("home");
        let provider = temp.path().join("claude");
        let ready = temp.path().join("ready");
        std::fs::write(&provider, "#!/bin/sh\nprintf ready > \"$1\"\nread ignored\n").expect("fixture");
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).expect("permissions");
        let child = DisposableChild(
            Command::new(&provider)
                .arg(&ready)
                .args(["--session-id", "session"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("disposable provider"),
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "fixture did not initialize");
            std::thread::sleep(Duration::from_millis(5));
        }
        let registry = temp.path().join(".claude/sessions");
        std::fs::create_dir_all(&registry).expect("registry");
        std::fs::write(
            registry.join("stale.json"),
            r#"{"sessionId":"session","pid":4294967295}"#,
        )
        .expect("stale");
        std::fs::write(
            registry.join("live.json"),
            format!(r#"{{"sessionId":"session","pid":{}}}"#, child.0.id()),
        )
        .expect("live");
        assert_eq!(
            session_presence(temp.path(), "session").expect("identity"),
            Presence::Live
        );
        assert_process_instance_identity(temp.path(), &registry, child.0.id());
    }

    #[cfg(target_os = "linux")]
    fn assert_process_instance_identity(home: &Path, registry: &Path, pid: u32) {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("process stat");
        let started = stat
            .rsplit_once(')')
            .expect("process name")
            .1
            .split_whitespace()
            .nth(19)
            .expect("start time");
        let machine = std::fs::read_to_string("/etc/machine-id").expect("machine ID");
        let namespace = std::fs::read_link("/proc/self/ns/pid").expect("PID namespace");
        let domain = format!("linux:{}:{}", machine.trim(), namespace.display());
        for (started, domain, expected_live) in [
            (started, domain.as_str(), true),
            ("0", domain.as_str(), false),
            (started, "different-machine-or-namespace", false),
        ] {
            std::fs::write(
                registry.join("live.json"),
                serde_json::to_vec(&serde_json::json!({
                    "sessionId": "session", "pid": pid, "procStart": started, "pidDomain": domain
                }))
                .expect("registry JSON"),
            )
            .expect("process identity");
            if expected_live {
                assert_eq!(
                    session_presence(home, "session").expect("exact instance"),
                    Presence::Live
                );
            } else {
                assert!(session_presence(home, "session").is_err());
            }
        }
    }
}
