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
            let owned_pid = self.terminal().and_then(crate::terminal::Terminal::owned_process_id);
            self.check_work_session_exit(owned_pid)?;
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
        } else if self.kind == PanelKind::Pi {
            saved_pi_history(&binding.session_id).is_some()
        } else if self.kind == PanelKind::Grok {
            saved_grok_history(&binding.session_id).is_some()
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
            self.check_work_session_exit(None)?;
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

    fn check_work_session_exit(&self, owned_pid: Option<u32>) -> Result<()> {
        if self.kind == PanelKind::Claude
            && self
                .session_binding
                .as_ref()
                .is_some_and(|binding| competing_session(&binding.session_id, owned_pid) != Some(false))
        {
            return Err(Error::State(
                "This conversation may still be open in another process.".into(),
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

fn competing_session(session: &str, owned_pid: Option<u32>) -> Option<bool> {
    use std::io::Read;

    let directory = crate::local_store::user_home_dir()?.join(".claude/sessions");
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    for (index, entry) in entries.enumerate() {
        if index >= 4096 {
            return None;
        }
        let path = entry.ok()?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > 64 * 1024 {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        if value.get("sessionId")?.as_str()? != session {
            continue;
        }
        let pid = u32::try_from(value.get("pid")?.as_u64()?).ok().filter(|pid| *pid > 0)?;
        if Some(pid) != owned_pid && process_may_be_alive(pid) {
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(unix)]
fn process_may_be_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_none_or(|pid| rustix::process::test_kill_process(pid) != Err(rustix::io::Errno::SRCH))
}

#[cfg(not(unix))]
fn process_may_be_alive(_pid: u32) -> bool {
    true
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
    let mut has_message = false;
    // Initial mode and file-history rows are metadata, not resumable messages.
    for index in 0..128 {
        line.clear();
        let bytes = u64::try_from(reader.read_until(b'\n', &mut line).ok()?).ok()?;
        if bytes == 0 {
            break;
        }
        remaining = remaining.checked_sub(bytes)?;
        let value: serde_json::Value = serde_json::from_slice(&line).ok()?;
        if index == 0 && value.get("isSidechain").and_then(serde_json::Value::as_bool) == Some(true) {
            return None;
        }
        if value.get("sessionId").is_some_and(|id| id.as_str() != Some(session_id)) {
            return None;
        }
        let kind = value.get("type")?.as_str()?;
        if matches!(kind, "user" | "assistant") {
            let message = value.get("message")?;
            if value.get("sessionId")?.as_str()? != session_id
                || message.get("role")?.as_str()? != kind
                || !supported_message_content(message.get("content")?)
            {
                return None;
            }
            has_message = true;
        }
    }
    has_message.then_some(())
}

fn supported_message_content(content: &serde_json::Value) -> bool {
    content.is_string()
        || content.as_array().is_some_and(|blocks| {
            !blocks.is_empty()
                && blocks.iter().all(|block| {
                    let string = |key| block.get(key).is_some_and(serde_json::Value::is_string);
                    match block.get("type").and_then(serde_json::Value::as_str) {
                        Some("text") => string("text"),
                        Some("thinking") => string("thinking") && string("signature"),
                        Some("tool_reference") => string("tool_name"),
                        Some("search_result") => {
                            string("source")
                                && string("title")
                                && block
                                    .get("content")
                                    .and_then(serde_json::Value::as_array)
                                    .is_some_and(|parts| {
                                        parts.iter().all(|part| {
                                            part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                                                && part.get("text").is_some_and(serde_json::Value::is_string)
                                        })
                                    })
                        }
                        Some("redacted_thinking") => string("data"),
                        Some("image" | "document") => block.get("source").is_some_and(supported_content_source),
                        Some("tool_use") => {
                            string("id")
                                && string("name")
                                && block.get("input").is_some_and(serde_json::Value::is_object)
                        }
                        Some("tool_result") => {
                            string("tool_use_id")
                                && block.get("is_error").is_none_or(serde_json::Value::is_boolean)
                                && block.get("content").is_none_or(supported_tool_result_content)
                        }
                        _ => false,
                    }
                })
        })
}

fn supported_tool_result_content(content: &serde_json::Value) -> bool {
    content.is_string()
        || content.as_array().is_some_and(|parts| {
            parts.iter().all(|part| {
                matches!(
                    part.get("type").and_then(serde_json::Value::as_str),
                    Some("text" | "image" | "document" | "search_result" | "tool_reference")
                )
            }) && (parts.is_empty() || supported_message_content(content))
        })
}

fn supported_content_source(source: &serde_json::Value) -> bool {
    let string = |key| source.get(key).is_some_and(serde_json::Value::is_string);
    match source.get("type").and_then(serde_json::Value::as_str) {
        Some("base64" | "text") => string("data") && string("media_type"),
        Some("url") => string("url"),
        Some("file") => string("file_id"),
        Some("content") => source.get("content").is_some_and(supported_message_content),
        _ => false,
    }
}

fn saved_session_backing_exists(session: &crate::runtime_state::AgentSessionRecord) -> bool {
    match session.kind {
        PanelKind::Codex => saved_rollout_header(&session.session_id).is_some(),
        // This database is the authoritative store rather than a search index.
        PanelKind::OpenCode => true,
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
    let header = session_metadata(path, PanelKind::Codex, &mut (1024 * 1024))?;
    (header.get("payload")?.get("id")?.as_str()? == session_id).then_some(())
}

fn saved_grok_history(session_id: &str) -> Option<()> {
    use std::path::{Component, Path, PathBuf};

    let mut components = Path::new(session_id).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || session_id.contains(['/', '\\'])
    {
        return None;
    }
    let connection = crate::local_store::open_read_only_sqlite(&crate::local_store::grok_sessions_db_path()?).ok()?;
    let mut query = connection
        .prepare("SELECT substr(cwd, 1, 4097) FROM session_docs WHERE session_id = ?1 LIMIT 2")
        .ok()?;
    let mut rows = query.query([session_id]).ok()?;
    let cwd: String = rows.next().ok()??.get(0).ok()?;
    if cwd.len() > 4096 || rows.next().ok()?.is_some() {
        return None;
    }
    let cwd = Path::new(cwd.trim()).components().collect::<PathBuf>();
    let directory = crate::local_store::grok_home_dir()?.join("sessions");
    let mut candidate = None;
    for (index, entry) in std::fs::read_dir(directory).ok()?.enumerate() {
        if index >= 4096 {
            return None;
        }
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
        if candidate
            .replace(entry.path().join(session_id).join("chat_history.jsonl"))
            .is_some()
        {
            return None;
        }
    }
    let header = readable_session_header(&candidate?)?;
    let content = header.get("content")?;
    match header.get("type")?.as_str()? {
        "system" | "assistant" | "tool_result" => content.is_string(),
        "user" => {
            content.is_string()
                || content.as_array().is_some_and(|parts| {
                    !parts.is_empty()
                        && parts.iter().all(|part| {
                            part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                                && part.get("text").is_some_and(serde_json::Value::is_string)
                        })
                })
        }
        _ => false,
    }
    .then_some(())
}

fn saved_pi_history(session_id: &str) -> Option<()> {
    let root = crate::local_store::user_home_dir()?.join(".pi/agent/sessions");
    let mut entries_left = 4096_usize;
    let mut bytes_left = 32 * 1024 * 1024;
    let mut found = false;
    for directory in std::fs::read_dir(root).ok()? {
        entries_left = entries_left.checked_sub(1)?;
        let directory = directory.ok()?;
        if !directory.file_type().ok()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(directory.path()).ok()? {
            entries_left = entries_left.checked_sub(1)?;
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension == "jsonl") {
                let header = session_metadata(&entry.path(), PanelKind::Pi, &mut bytes_left);
                if let Some(header) = header
                    && header.get("id").and_then(serde_json::Value::as_str) == Some(session_id)
                {
                    if found
                        || ["parentSession", "parent_session"].iter().any(|key| {
                            header
                                .get(key)
                                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
                        })
                    {
                        return None;
                    }
                    found = true;
                }
            }
            if bytes_left == 0 {
                return None;
            }
        }
    }
    found.then_some(())
}

fn session_metadata(path: &std::path::Path, kind: PanelKind, budget: &mut u64) -> Option<serde_json::Value> {
    use std::io::{BufRead, BufReader, Read};

    if !path.metadata().ok()?.is_file() {
        return None;
    }
    let limit = (*budget).min(1024 * 1024);
    let mut reader = BufReader::new(std::fs::File::open(path).ok()?.take(limit + 1));
    let mut remaining = limit;
    let mut line = Vec::new();
    loop {
        line.clear();
        let count = u64::try_from(reader.read_until(b'\n', &mut line).ok()?).ok()?;
        *budget = budget.saturating_sub(count);
        remaining = remaining.checked_sub(count)?;
        if count == 0 {
            return None;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) else {
            continue;
        };
        let expected = if kind == PanelKind::Pi {
            "session"
        } else {
            "session_meta"
        };
        if value.get("type").and_then(serde_json::Value::as_str) == Some(expected) {
            return Some(value);
        }
        if kind == PanelKind::Pi {
            return None;
        }
    }
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

pub(super) fn shutdown_for_restart(terminal: &mut crate::terminal::Terminal, continuation: bool) -> Result<()> {
    if !terminal.shutdown_with_timeout(std::time::Duration::from_secs(2)) && continuation {
        return Err(Error::State(
            "The previous process has not finished shutting down. No continuation was started.".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
