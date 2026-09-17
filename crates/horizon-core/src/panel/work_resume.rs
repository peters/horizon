use crate::agent_work::WorkContinuation;
use crate::error::{Error, Result};

use super::{Panel, PanelKind};

pub(super) struct RestartWork {
    pub(super) owner: Option<std::sync::Arc<crate::agent_work::WorkOwner>>,
    pub(super) state: WorkContinuation,
}

impl Panel {
    /// Enable continuation on future launches. Disabling also removes pending
    /// offers and prevents cancellation for a continuation handoff at shutdown.
    pub fn set_work_resume_enabled(&mut self, enabled: bool) {
        self.work_resume.enabled = enabled;
        if !enabled && let Some(terminal) = self.terminal_mut() {
            terminal.work_owner = None;
            terminal.work_continuation = WorkContinuation::default();
        }
    }

    /// Authorize one explicit continuation; the ordinary restart path performs
    /// bounded teardown and submits the brief only to the same conversation.
    ///
    /// # Errors
    /// Rejects stale offers, ambiguous/custom launch commands, and providers
    /// without an exact session target.
    pub fn check_work_resume(&self) -> Result<()> {
        if !self.work_resume.enabled
            || self.remote_workspace.is_some()
            || self.launch_command.is_some()
            || !self.launch_args.is_empty()
            || !matches!(
                self.kind,
                PanelKind::Claude | PanelKind::Codex | PanelKind::OpenCode | PanelKind::Pi | PanelKind::Grok
            )
            || self
                .session_binding
                .as_ref()
                .is_none_or(|binding| binding.session_id.is_empty() || binding.kind != self.kind)
        {
            return Err(Error::State("This panel cannot safely restart into an exact conversation. Review it and continue from its terminal.".into()));
        }
        let terminal = self
            .terminal()
            .ok_or_else(|| Error::State("No terminal to resume".into()))?;
        if !terminal.work_continuation.owns_process {
            return Err(Error::State(
                "This process cannot be safely replaced. Continue from its terminal.".into(),
            ));
        }
        if terminal.pending_work_resume().is_none()
            || terminal.work_continuation.offered_session.as_deref()
                != self.session_binding.as_ref().map(|binding| binding.session_id.as_str())
        {
            return Err(Error::State(
                "The resume offer is no longer current. Review the terminal before continuing.".into(),
            ));
        }
        Ok(())
    }

    /// Authorize one continuation of the conversation that produced this offer.
    ///
    /// # Errors
    /// Returns the same refusal as [`Self::check_work_resume`].
    pub fn request_work_resume(&mut self) -> Result<()> {
        self.check_work_resume()?;
        let session_id = self
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.clone())
            .ok_or_else(|| Error::State("Missing conversation identity".into()))?;
        let terminal = self
            .terminal_mut()
            .ok_or_else(|| Error::State("No terminal to resume".into()))?;
        terminal.work_continuation.requested_session.get_or_insert(session_id);
        Ok(())
    }
    pub(super) fn preflight_restart_work(
        &self,
        program: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        brief: Option<&str>,
    ) -> Result<Option<String>> {
        if brief.is_some() {
            self.check_saved_work_session()?;
        }
        let owned = self
            .launch_args
            .is_empty()
            .then(|| self.work_launch().owned_command(program, args, env))
            .flatten();
        if brief.is_some() && owned.is_none() {
            return Err(Error::State(
                "The launch no longer resolves to an owned executable. Continue from its terminal.".into(),
            ));
        }
        if let Some(brief) = brief {
            self.append_work_brief(&mut args.to_vec(), brief)?;
        }
        Ok(owned)
    }

    fn check_saved_work_session(&self) -> Result<()> {
        use crate::runtime_state::{AgentSessionCatalog, PanelState, RuntimeState, WorkspaceState};

        let binding = self
            .session_binding
            .as_ref()
            .ok_or_else(|| Error::State("Missing conversation identity".into()))?;
        let exists = if self.kind == PanelKind::Claude {
            saved_claude_history(&binding.session_id).is_some()
        } else {
            // Scope discovery to this provider and preserve the confirmed exact
            // ID; bootstrap aliases must never silently retarget a continuation.
            let state = RuntimeState {
                workspaces: vec![WorkspaceState {
                    panels: vec![PanelState {
                        kind: self.kind,
                        session_binding: Some(binding.clone()),
                        ..PanelState::default()
                    }],
                    ..WorkspaceState::default()
                }],
                ..RuntimeState::default()
            };
            AgentSessionCatalog::load_for_runtime_state(&state)?
                .into_catalog()
                .recent_for(self.kind, None)
                .iter()
                .any(|session| session.session_id == binding.session_id && saved_session_backing_exists(session))
        };
        if !exists {
            return Err(Error::State(
                "The saved conversation could not be verified. Review it and continue from its terminal.".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn prepare_restart_work(
        &self,
        owned: Option<&str>,
        args: &mut Vec<String>,
        env: &mut std::collections::HashMap<String, String>,
        brief: Option<&str>,
    ) -> Result<RestartWork> {
        if brief.is_some() {
            self.check_work_session_exit()?;
        }
        let work_owner = self.work_launch().attach(owned, args, env);
        let work_state = self.prepare_work_process(args, owned);
        if let Some(brief) = brief {
            self.append_work_brief(args, brief)?;
        }
        Ok(RestartWork {
            owner: work_owner,
            state: work_state,
        })
    }

    fn work_launch(&self) -> crate::agent_work::WorkLaunch<'_> {
        crate::agent_work::WorkLaunch {
            panel: &self.local_id,
            kind: self.kind,
            policy: &self.work_resume,
            cwd: self.launch_cwd.as_deref(),
            session_id: self.session_binding.as_ref().map(|binding| binding.session_id.as_str()),
            default_command: self.launch_command.is_none(),
        }
    }

    pub(super) fn prepare_work_process(&self, args: &mut [String], owned: Option<&str>) -> WorkContinuation {
        let mut state = WorkContinuation::default();
        if self.work_resume.enabled
            && self.launch_command.is_none()
            && self.launch_args.is_empty()
            && self.kind.is_agent()
        {
            state.own_process(args, owned);
        }
        state
    }

    pub(super) fn requested_work_brief(&mut self) -> Result<Option<String>> {
        let Some(requested) = self
            .terminal_mut()
            .and_then(|terminal| terminal.work_continuation.requested_session.take())
        else {
            return Ok(None);
        };
        if self
            .session_binding
            .as_ref()
            .is_none_or(|binding| binding.session_id != requested)
        {
            return Err(Error::State(
                "The conversation changed after Resume work was selected.".into(),
            ));
        }
        self.check_work_resume()?;
        Ok(Some("The user selected Resume work in Horizon for this conversation. Re-read the latest user request and current conversation, then continue only the work already authorized. Re-verify the working tree, tool side effects, and background processes; interrupted tools may have partially completed. This does not answer pending questions or grant tool permissions. Ask the user if approval or a decision is still needed.".into()))
    }

    fn check_work_session_exit(&self) -> Result<()> {
        if self.kind == PanelKind::Claude
            && self
                .session_binding
                .as_ref()
                .is_some_and(|binding| crate::runtime_state::live_claude_session_ids().contains(&binding.session_id))
        {
            return Err(Error::State(
                "This conversation is still open in another process.".into(),
            ));
        }
        Ok(())
    }

    fn append_work_brief(&self, args: &mut Vec<String>, brief: &str) -> Result<()> {
        if !crate::agent_work::append_seed(self.kind, args, brief) {
            return Err(Error::State(
                "This launch cannot accept a continuation prompt. Continue from its terminal.".into(),
            ));
        }
        Ok(())
    }
}

fn saved_claude_history(session_id: &str) -> Option<()> {
    use std::io::{BufRead, BufReader, Read};

    const MAX_PREFIX_BYTES: u64 = 1024 * 1024;
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let projects = crate::local_store::user_home_dir()?.join(".claude/projects");
    let mut transcript = None;
    for (index, entry) in std::fs::read_dir(projects).ok()?.enumerate() {
        if index >= 4096 {
            return None;
        }
        let path = entry.ok()?.path().join(format!("{session_id}.jsonl"));
        if path.is_file() && transcript.replace(path).is_some() {
            return None;
        }
    }
    let mut reader = BufReader::new(std::fs::File::open(transcript?).ok()?.take(MAX_PREFIX_BYTES + 1));
    let mut line = Vec::new();
    let mut remaining = MAX_PREFIX_BYTES;
    // Initial mode and file-history rows are metadata, not resumable messages.
    for _ in 0..128 {
        line.clear();
        let bytes = u64::try_from(reader.read_until(b'\n', &mut line).ok()?).ok()?;
        remaining = remaining.checked_sub(bytes)?;
        let value: serde_json::Value = serde_json::from_slice(&line).ok()?;
        if value.get("sessionId").is_some_and(|id| id.as_str() != Some(session_id)) {
            return None;
        }
        if matches!(value.get("type")?.as_str()?, "user" | "assistant") {
            return (value.get("sessionId")?.as_str()? == session_id
                && value
                    .get("message")?
                    .get("content")
                    .is_some_and(|content| content.is_string() || content.is_array()))
            .then_some(());
        }
    }
    None
}

fn saved_session_backing_exists(session: &crate::runtime_state::AgentSessionRecord) -> bool {
    match session.kind {
        PanelKind::Codex => saved_rollout_header(&session.session_id).is_some(),
        PanelKind::Grok => saved_grok_history(session).is_some(),
        // The Pi catalog reads the session file; the OpenCode database is its
        // authoritative session store rather than a separate search index.
        PanelKind::Pi | PanelKind::OpenCode => true,
        _ => false,
    }
}

fn saved_rollout_header(session_id: &str) -> Option<()> {
    let connection = crate::local_store::open_read_only_sqlite(&crate::local_store::codex_db_path()?).ok()?;
    let path: String = connection
        .query_row(
            "SELECT substr(rollout_path, 1, 4097) FROM threads WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    if path.len() > 4096 {
        return None;
    }
    let path = std::path::Path::new(&path);
    if !path.is_absolute() {
        return None;
    }
    let header = readable_session_header(path)?;
    (header.get("type")?.as_str()? == "session_meta" && header.get("payload")?.get("id")?.as_str()? == session_id)
        .then_some(())
}

fn saved_grok_history(session: &crate::runtime_state::AgentSessionRecord) -> Option<()> {
    use std::path::{Component, Path, PathBuf};

    let mut components = Path::new(&session.session_id).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || session.session_id.contains(['/', '\\'])
    {
        return None;
    }
    let cwd = Path::new(session.cwd.as_deref()?);
    let directory = crate::local_store::grok_home_dir()?.join("sessions");
    for entry in std::fs::read_dir(directory).ok()?.take(4096) {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let mut decoded = url::form_urlencoded::parse(name.to_str()?.as_bytes());
        let Some((decoded_cwd, value)) = decoded.next() else {
            continue;
        };
        if !value.is_empty()
            || decoded.next().is_some()
            || Path::new(decoded_cwd.as_ref()).components().collect::<PathBuf>() != cwd
        {
            continue;
        }
        let header = readable_session_header(&entry.path().join(&session.session_id).join("chat_history.jsonl"))?;
        header.get("type")?.as_str()?;
        header.get("content")?;
        return Some(());
    }
    None
}

fn readable_session_header(path: &std::path::Path) -> Option<serde_json::Value> {
    use std::io::{BufRead, BufReader, Read};

    const MAX_HEADER_BYTES: u64 = 1024 * 1024;
    if !path.metadata().ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    BufReader::new(file.take(MAX_HEADER_BYTES + 1))
        .read_until(b'\n', &mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_HEADER_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub(super) fn shutdown_for_restart(terminal: &mut crate::terminal::Terminal) -> Result<()> {
    if !terminal.shutdown_with_timeout(std::time::Duration::from_secs(2)) {
        return Err(Error::State(
            "The previous process has not finished shutting down. No continuation was started.".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::agent_work::{ResumePolicy, StartupPlan, WorkLaunch};
    use crate::runtime_state::AgentSessionBinding;
    use crate::{PanelId, PanelOptions, WorkspaceId};

    #[test]
    fn confirmed_restart_replaces_the_process_and_submits_the_brief_once() {
        const CHILD_ENV: &str = "HORIZON_TEST_MANUAL_RESTART_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            exercise_confirmed_restart();
            return;
        }
        let home = tempfile::tempdir().expect("isolated home");
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).expect("fixture bin");
        std::fs::write(home.path().join(".bashrc"), "").expect("isolated shell config");
        let provider = bin.join("pi");
        std::fs::write(
            &provider,
            r#"#!/bin/bash
if test -f "$HOME/provider.pid"; then
    read -r previous < "$HOME/provider.pid"
    if kill -0 "$previous" 2>/dev/null; then
        printf 'overlapping processes\n' > "$HOME/overlap"
    fi
fi
printf '%s\0' "$@" > "$HOME/args.$$"
printf '%s\n' "$$" > "$HOME/provider.pid"
trap 'exit 0' HUP TERM
printf '%s\n' "$$" >> "$HOME/launches"
while :; do read -r -t 1 || :; done
"#,
        )
        .expect("fixture executable");
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).expect("executable permission");
        std::fs::copy(&provider, bin.join("claude")).expect("second disposable provider");
        // Re-exec isolates HOME, PATH and SHELL from concurrent tests and ensures
        // the ordinary launch resolver can only find our disposable provider.
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                concat!(
                    module_path!(),
                    "::confirmed_restart_replaces_the_process_and_submits_the_brief_once"
                )
                .trim_start_matches("horizon_core::"),
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD_ENV, "1")
            .env("HOME", home.path())
            .env("SHELL", "/bin/bash")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .current_dir(home.path())
            .output()
            .expect("isolated test process");
        assert!(
            output.status.success(),
            "isolated restart failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let launches = std::fs::read_to_string(home.path().join("launches")).expect("test child executed");
        assert_eq!(launches.lines().count(), 5);
        assert!(
            !home.path().join("overlap").exists(),
            "replacement started before previous process exited"
        );
    }

    fn exercise_confirmed_restart() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("fixture home"));
        let saved_session = write_pi_session(&home);
        let mut panel = restored_provider(&home, PanelKind::Pi);
        assert_eq!(wait_for_launch(&home, 1), ["--session", "fixture-session"]);
        panel.check_saved_work_session().expect("catalog-backed session");
        exercise_catalog_rows(&mut panel, &home);
        assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
            std::fs::remove_file(saved_session).expect("delete saved session after confirmation");
        });
        let _ = write_pi_session(&home);
        assert_refusal_preserves_process(&mut panel, &home, "owned executable", || {
            std::fs::write(home.join(".bashrc"), "pi() { :; }").expect("replace executable resolution");
        });
        std::fs::write(home.join(".bashrc"), "").expect("restore executable resolution");
        panel.request_work_resume().expect("confirm offered continuation");
        panel.restart().expect("confirmed restart");
        let args = wait_for_launch(&home, 2);
        assert_eq!(args.len(), 3, "exactly one brief must be submitted");
        assert_eq!(&args[..2], ["--session", "fixture-session"]);
        assert!(args[2].starts_with("The user selected Resume work in Horizon for this conversation."));
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
        assert!(panel.requested_work_brief().expect("consumed authorization").is_none());
        panel.restart().expect("ordinary restart after continuation");
        assert_eq!(wait_for_launch(&home, 3), ["--session", "fixture-session"]);
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close disposable provider");

        let mut panel = restored_provider(&home, PanelKind::Claude);
        let _ = wait_for_launch(&home, 4);
        let project = home.join(".claude/projects/fixture");
        std::fs::create_dir_all(&project).expect("transcript directory");
        let transcript = project.join("fixture-session.jsonl");
        let valid = concat!(
            "{\"type\":\"mode\",\"sessionId\":\"fixture-session\",\"mode\":\"normal\"}\n",
            "{\"type\":\"file-history-snapshot\",\"snapshot\":{}}\n",
            "{\"type\":\"user\",\"sessionId\":\"fixture-session\",\"message\":{\"role\":\"user\",\"content\":\"Fixture request\"}}\n"
        );
        std::fs::write(&transcript, valid).expect("saved transcript");
        panel.check_saved_work_session().expect("metadata-prefixed history");
        assert!(crate::runtime_state::claude_session_transcript_exists(
            "fixture-session"
        ));
        assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
            std::fs::remove_file(&transcript).expect("remove transcript after confirmation");
        });
        for invalid in [
            String::new(),
            "{invalid}\n".into(),
            "{}\n".into(),
            valid.replace("fixture-session", "other-session"),
        ] {
            std::fs::write(&transcript, valid).expect("restore valid history");
            assert_refusal_preserves_process(&mut panel, &home, "saved conversation", || {
                std::fs::write(&transcript, invalid).expect("invalidate history after confirmation");
            });
        }
        std::fs::write(&transcript, valid).expect("repair history");
        panel.request_work_resume().expect("confirm repaired history");
        panel.restart().expect("resume repaired history");
        let args = wait_for_launch(&home, 5);
        assert_eq!(&args[..2], ["--resume", "fixture-session"]);
        assert_eq!(
            args.iter()
                .filter(|arg| arg.starts_with("The user selected Resume work"))
                .count(),
            1
        );
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close repaired-history provider");
    }

    fn write_pi_session(home: &std::path::Path) -> std::path::PathBuf {
        let directory = home.join(".pi/agent/sessions/fixture");
        std::fs::create_dir_all(&directory).expect("session directory");
        let path = directory.join("fixture-session.jsonl");
        let session = serde_json::json!({
            "type": "session", "id": "fixture-session", "cwd": home,
            "timestamp": "2026-01-01T00:00:00.000Z", "version": 3
        });
        std::fs::write(&path, format!("{session}\n")).expect("saved session");
        path
    }

    fn exercise_catalog_rows(panel: &mut Panel, home: &std::path::Path) {
        for (kind, path, table, schema) in [
            (
                PanelKind::Codex,
                home.join(".codex/state_5.sqlite"),
                "threads",
                "CREATE TABLE threads (id TEXT, rollout_path TEXT, source TEXT, title TEXT, cwd TEXT, updated_at INTEGER, archived INTEGER);
                 INSERT INTO threads VALUES ('fixture-session', NULL, 'cli', 'Fixture', '/repo', 1, 0);",
            ),
            (
                PanelKind::OpenCode,
                crate::opencode_paths::opencode_db_path().expect("session DB path"),
                "session",
                "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_updated INTEGER, time_archived INTEGER, parent_id TEXT);
                 INSERT INTO session VALUES ('fixture-session', 'Fixture', '/repo', 1, NULL, NULL);",
            ),
            (
                PanelKind::Grok,
                crate::local_store::grok_sessions_db_path().expect("session DB path"),
                "session_docs",
                "CREATE TABLE session_docs (session_id TEXT, cwd TEXT, updated_at INTEGER, title TEXT);
                 INSERT INTO session_docs VALUES ('fixture-session', '/repo', 1, 'Fixture');",
            ),
        ] {
            panel.kind = kind;
            panel.session_binding.as_mut().expect("binding").kind = kind;
            // The matching Pi ID must not satisfy another provider's lookup.
            assert!(panel.check_saved_work_session().is_err(), "{kind:?} missing store");
            std::fs::create_dir_all(path.parent().expect("DB directory")).expect("DB directory");
            let connection = rusqlite::Connection::open(&path).expect("fixture DB");
            connection.execute_batch(schema).expect("saved session row");
            let backing = match kind {
                PanelKind::Codex => {
                    assert!(panel.check_saved_work_session().is_err(), "NULL rollout path");
                    let path = home.join(".codex/sessions/fixture.jsonl");
                    connection.execute("UPDATE threads SET rollout_path = ?1", [path.to_str().expect("path")]).expect("rollout path");
                    Some((path, serde_json::json!({"type": "session_meta", "payload": {"id": "fixture-session"}})))
                }
                PanelKind::Grok => Some((
                    home.join(".grok/sessions/%2Frepo/fixture-session/chat_history.jsonl"),
                    serde_json::json!({"type": "user", "content": "Fixture request"}),
                )),
                _ => None,
            };
            if let Some((path, header)) = &backing {
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} stale index without backing file");
                std::fs::create_dir_all(path.parent().expect("session directory")).expect("session directory");
                std::fs::write(path, format!("{header}\n")).expect("saved transcript");
                panel.check_saved_work_session().expect("indexed readable transcript");
                assert_refusal_preserves_process(panel, home, "saved conversation", || {
                    std::fs::remove_file(path).expect("remove transcript while retaining index row");
                });
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} deleted backing file");
                std::fs::write(path, "").expect("empty saved transcript");
                assert!(panel.check_saved_work_session().is_err(), "{kind:?} empty backing file");
                std::fs::write(path, format!("{header}\n")).expect("restore saved transcript");
            }
            panel.check_saved_work_session().expect("provider-scoped saved session");
            connection.execute(&format!("DELETE FROM {table}"), []).expect("delete session");
            assert!(panel.check_saved_work_session().is_err(), "{kind:?} deleted session");
        }
        panel.kind = PanelKind::Pi;
        panel.session_binding.as_mut().expect("binding").kind = PanelKind::Pi;
    }

    fn assert_refusal_preserves_process(
        panel: &mut Panel,
        home: &std::path::Path,
        reason: &str,
        invalidate: impl FnOnce(),
    ) {
        let original_pid = std::fs::read_to_string(home.join("provider.pid")).expect("original PID");
        let launches = std::fs::read(home.join("launches")).expect("original launches");
        panel
            .request_work_resume()
            .expect("confirm before target becomes unavailable");
        invalidate();
        let error = panel.restart().expect_err("unavailable continuation must be refused");
        assert!(error.to_string().contains(reason), "unexpected refusal: {error}");
        assert!(
            std::process::Command::new("/bin/bash")
                .args(["-c", "kill -0 \"$1\"", "fixture-probe", original_pid.trim()])
                .status()
                .expect("probe original process")
                .success(),
            "refused continuation shut down the original process"
        );
        assert_eq!(std::fs::read(home.join("launches")).expect("launches"), launches);
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_some());
        assert!(
            panel
                .requested_work_brief()
                .expect("refused authorization consumed")
                .is_none()
        );
    }

    fn restored_provider(home: &std::path::Path, kind: PanelKind) -> Panel {
        Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind,
                cwd: Some(home.to_path_buf()),
                local_id: Some("manual-restart-fixture".into()),
                is_restore: true,
                work_resume: ResumePolicy {
                    enabled: true,
                    ..Default::default()
                },
                session_binding: Some(AgentSessionBinding::new(
                    kind,
                    "fixture-session".into(),
                    None,
                    None,
                    None,
                )),
                ..Default::default()
            },
        )
        .expect("restored disposable provider")
    }

    fn wait_for_launch(home: &std::path::Path, expected: usize) -> Vec<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let launches = std::fs::read_to_string(home.join("launches")).unwrap_or_default();
            let pids: Vec<_> = launches.lines().collect();
            assert!(pids.len() <= expected, "unexpected additional provider launch");
            if pids.len() == expected && launches.ends_with('\n') {
                let bytes = std::fs::read(home.join(format!("args.{}", pids[expected - 1]))).expect("provider args");
                return bytes
                    .strip_suffix(&[0])
                    .expect("NUL terminated arguments")
                    .split(|byte| *byte == 0)
                    .map(|arg| String::from_utf8(arg.to_vec()).expect("UTF-8 argument"))
                    .collect();
            }
            assert!(std::time::Instant::now() < deadline, "provider did not launch");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn fixture() -> Panel {
        let mut panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "cat >/dev/null".into()],
                ..Default::default()
            },
        )
        .expect("disposable shell");
        panel.kind = PanelKind::Pi;
        panel.launch_command = None;
        panel.launch_args.clear();
        panel.session_binding = Some(AgentSessionBinding {
            kind: PanelKind::Pi,
            session_id: "fixture-session".into(),
            cwd: None,
            label: None,
            updated_at: None,
        });
        panel.work_resume = ResumePolicy {
            enabled: true,
            ..Default::default()
        };
        let plan = StartupPlan::prepare(
            &WorkLaunch {
                panel: &panel.local_id,
                kind: panel.kind,
                policy: &panel.work_resume,
                cwd: None,
                session_id: Some("fixture-session"),
                default_command: true,
            },
            true,
            true,
        );
        panel.terminal_mut().expect("terminal").work_continuation = plan.state;
        panel.terminal_mut().expect("terminal").work_continuation.owns_process = true;
        panel
    }
    #[test]
    fn manual_request_is_revoked_by_user_input_or_disabling_policy() {
        let mut panel = fixture();
        assert!(panel.request_work_resume().is_ok());
        assert!(panel.requested_work_brief().expect("brief").is_some());
        panel.request_work_resume().expect("request before cancellation");
        panel.terminal().expect("terminal").write_input(b"new request\n");
        assert!(panel.requested_work_brief().is_err());
        assert!(panel.requested_work_brief().expect("consumed refusal").is_none());
        panel.set_work_resume_enabled(false);
        assert!(panel.terminal().expect("terminal").pending_work_resume().is_none());
        assert!(panel.request_work_resume().is_err());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
    #[test]
    fn custom_arguments_and_missing_bindings_cannot_target_a_different_conversation() {
        let mut panel = fixture();
        panel.launch_args = vec!["--session".into(), "different".into()];
        assert!(panel.request_work_resume().is_err());
        panel.launch_args.clear();
        panel.session_binding = None;
        assert!(panel.request_work_resume().is_err());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
    #[test]
    fn changing_the_binding_revokes_the_offer_and_consumes_queued_authorization() {
        let mut panel = fixture();
        panel.request_work_resume().expect("request");
        panel.session_binding.as_mut().expect("binding").session_id = "different-session".into();
        assert!(panel.check_work_resume().is_err());
        assert!(panel.request_work_resume().is_err());
        assert!(panel.requested_work_brief().is_err());
        assert!(panel.requested_work_brief().expect("refusal consumed").is_none());
        shutdown_for_restart(panel.terminal_mut().expect("terminal")).expect("close fixture");
    }
}
