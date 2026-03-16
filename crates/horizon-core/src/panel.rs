use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use uuid::Uuid;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, new_local_id};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::transcript::PanelTranscript;
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;

const DEFAULT_CELL_WIDTH: u16 = 8;
const DEFAULT_CELL_HEIGHT: u16 = 17;

pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];
const DEFAULT_PANEL_SCROLLBACK_LIMIT: usize = 24_000;
const AGENT_PANEL_SCROLLBACK_LIMIT: usize = 24_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Shell,
    Codex,
    Claude,
    Command,
    Editor,
    GitChanges,
    Usage,
}

impl PanelKind {
    #[must_use]
    pub fn is_agent(self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Command => "Command",
            Self::Editor => "Editor",
            Self::GitChanges => "Git Changes",
            Self::Usage => "Usage",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
pub enum PanelResume {
    #[default]
    #[serde(rename = "fresh")]
    Fresh,
    #[serde(rename = "last")]
    Last,
    #[serde(rename = "session")]
    Session { session_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelLayout {
    pub position: [f32; 2],
    pub size: [f32; 2],
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            size: DEFAULT_PANEL_SIZE,
        }
    }
}

pub struct PanelOptions {
    pub name: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub local_id: Option<String>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    pub transcript_root: Option<PathBuf>,
}

impl Default for PanelOptions {
    fn default() -> Self {
        Self {
            name: None,
            command: None,
            args: Vec::new(),
            cwd: None,
            rows: 24,
            cols: 80,
            kind: PanelKind::default(),
            resume: PanelResume::default(),
            position: None,
            size: None,
            local_id: None,
            session_binding: None,
            template: None,
            transcript_root: None,
        }
    }
}

pub struct Panel {
    pub id: PanelId,
    pub local_id: String,
    pub title: String,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub layout: PanelLayout,
    pub workspace_id: WorkspaceId,
    pub content: PanelContent,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    pub launched_at_millis: i64,
    has_custom_name: bool,
    /// Original launch command (for persistence).
    pub launch_command: Option<String>,
    /// Original launch args (for persistence).
    pub launch_args: Vec<String>,
    /// Working directory (for persistence).
    pub launch_cwd: Option<PathBuf>,
}

impl Panel {
    /// Convenience accessor for the terminal content (if this panel holds one).
    #[must_use]
    pub fn terminal(&self) -> Option<&Terminal> {
        self.content.terminal()
    }

    /// Mutable accessor for the terminal content.
    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.content.terminal_mut()
    }

    /// Convenience accessor for the editor content (if this panel holds one).
    #[must_use]
    pub fn editor(&self) -> Option<&MarkdownEditor> {
        self.content.editor()
    }

    /// Mutable accessor for the editor content.
    pub fn editor_mut(&mut self) -> Option<&mut MarkdownEditor> {
        self.content.editor_mut()
    }

    /// Convenience accessor for the git changes content (if this panel holds one).
    #[must_use]
    pub fn git_changes(&self) -> Option<&crate::git_changes::GitChangesViewer> {
        self.content.git_changes()
    }

    /// Mutable accessor for the git changes content.
    pub fn git_changes_mut(&mut self) -> Option<&mut crate::git_changes::GitChangesViewer> {
        self.content.git_changes_mut()
    }
}

impl Panel {
    /// Spawn a new panel — either a PTY-backed terminal or a markdown editor.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal runtime cannot be created.
    pub fn spawn(id: PanelId, workspace_id: WorkspaceId, opts: PanelOptions) -> Result<Self> {
        let PanelOptions {
            name,
            command,
            args,
            cwd,
            rows,
            cols,
            kind,
            resume,
            position,
            size,
            local_id,
            session_binding,
            template,
            transcript_root,
        } = opts;

        let local_id = local_id.unwrap_or_else(new_local_id);

        if kind == PanelKind::Editor {
            return Self::spawn_editor(id, workspace_id, local_id, name, command, position, size, template);
        }

        if kind == PanelKind::GitChanges {
            return Self::spawn_git_changes(id, workspace_id, local_id, name, position, size, template, cwd);
        }

        if kind == PanelKind::Usage {
            return Self::spawn_usage(id, workspace_id, local_id, name, position, size, template);
        }

        let (transcript, replay_bytes) = prepare_transcript_restore(id, kind, transcript_root, &local_id);

        // Save original launch params for persistence before they're
        // transformed by resolve_launch_command.
        let saved_command = command.clone();
        let saved_args = args.clone();
        let saved_cwd = cwd.clone();
        let saved_cwd_string = saved_cwd.as_ref().map(|path| path.display().to_string());
        let (session_binding, should_resume_binding) = resolve_session_binding(
            kind,
            &resume,
            session_binding,
            saved_cwd_string.as_deref(),
            name.as_deref(),
        );

        let (program, launch_args) = resolve_launch_command(
            command,
            args,
            kind,
            &resume,
            session_binding.as_ref(),
            should_resume_binding,
        );

        if kind.is_agent() {
            tracing::info!(
                panel_id = id.0,
                kind = ?kind,
                resume = ?resume,
                session_id = session_binding.as_ref().map(|b| b.session_id.as_str()),
                should_resume = should_resume_binding,
                cwd = saved_cwd_string.as_deref(),
                cmd = %format!("{program} {}", launch_args.join(" ")),
                "launching agent panel"
            );
        }

        let (program, launch_args) = if let Some(transcript) = transcript.as_ref() {
            transcript.wrap_launch_command(program, launch_args)
        } else {
            (program, launch_args)
        };
        let env = agent_env(kind);
        let has_custom_name = name.is_some();
        let title = name.unwrap_or_else(|| format!("Terminal {}", id.0));
        let terminal = Terminal::spawn(TerminalSpawnOptions {
            program,
            args: launch_args,
            cwd,
            rows,
            cols,
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            scrollback_limit: scrollback_limit_for_kind(kind),
            window_id: id.0,
            replay_bytes,
            env,
        })?;

        tracing::info!("created panel '{}' (id={})", title, id.0);

        Ok(Self {
            id,
            local_id,
            title,
            kind,
            resume,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            content: PanelContent::Terminal(terminal),
            session_binding,
            template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            launch_command: saved_command,
            launch_args: saved_args,
            launch_cwd: saved_cwd,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_editor(
        id: PanelId,
        workspace_id: WorkspaceId,
        local_id: String,
        name: Option<String>,
        command: Option<String>,
        position: Option<[f32; 2]>,
        size: Option<[f32; 2]>,
        template: Option<PanelTemplateRef>,
    ) -> Result<Self> {
        let editor = if let Some(ref path_str) = command {
            let path = PathBuf::from(path_str);
            if path.exists() {
                MarkdownEditor::open(path)?
            } else {
                let mut ed = MarkdownEditor::scratch();
                ed.file_path = Some(path);
                ed
            }
        } else {
            MarkdownEditor::scratch()
        };

        let has_custom_name = name.is_some();
        let title = name.unwrap_or_else(|| {
            command
                .as_deref()
                .and_then(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "Markdown".to_string())
        });

        tracing::info!("created editor panel '{}' (id={})", title, id.0);

        Ok(Self {
            id,
            local_id,
            title,
            kind: PanelKind::Editor,
            resume: PanelResume::Fresh,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            content: PanelContent::Editor(editor),
            session_binding: None,
            template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            launch_command: command,
            launch_args: Vec::new(),
            launch_cwd: None,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
    fn spawn_git_changes(
        id: PanelId,
        workspace_id: WorkspaceId,
        local_id: String,
        name: Option<String>,
        position: Option<[f32; 2]>,
        size: Option<[f32; 2]>,
        template: Option<PanelTemplateRef>,
        cwd: Option<PathBuf>,
    ) -> Result<Self> {
        let has_custom_name = name.is_some();
        let title = name.unwrap_or_else(|| "Git Changes".to_string());
        tracing::info!("created git changes panel '{}' (id={})", title, id.0);
        Ok(Self {
            id,
            local_id,
            title,
            kind: PanelKind::GitChanges,
            resume: PanelResume::Fresh,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            content: PanelContent::GitChanges(crate::git_changes::GitChangesViewer::new()),
            session_binding: None,
            template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            launch_command: None,
            launch_args: Vec::new(),
            launch_cwd: cwd,
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    fn spawn_usage(
        id: PanelId,
        workspace_id: WorkspaceId,
        local_id: String,
        name: Option<String>,
        position: Option<[f32; 2]>,
        size: Option<[f32; 2]>,
        template: Option<PanelTemplateRef>,
    ) -> Result<Self> {
        let has_custom_name = name.is_some();
        let title = name.unwrap_or_else(|| "Usage".to_string());
        tracing::info!("created usage panel '{}' (id={})", title, id.0);
        Ok(Self {
            id,
            local_id,
            title,
            kind: PanelKind::Usage,
            resume: PanelResume::Fresh,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            content: PanelContent::Usage(UsageDashboard::new()),
            session_binding: None,
            template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            launch_command: None,
            launch_args: Vec::new(),
            launch_cwd: None,
        })
    }

    /// Drain pending terminal events. Returns `true` if any output was processed.
    pub fn process_output(&mut self) -> bool {
        let Some(terminal) = self.content.terminal_mut() else {
            return false;
        };
        let had_output = terminal.process_events();

        if had_output && !self.has_custom_name {
            let title = terminal.title();
            if !title.is_empty() {
                self.title = title.to_string();
            }
        }
        had_output
    }

    #[must_use]
    pub fn child_exited(&self) -> bool {
        self.content.terminal().is_some_and(Terminal::child_exited)
    }

    /// Returns `true` if the terminal bell has fired since the last call.
    pub fn take_bell(&mut self) -> bool {
        self.content.terminal_mut().is_some_and(Terminal::take_bell)
    }

    pub fn take_notification(&mut self) -> Option<crate::terminal::AgentNotification> {
        self.content.terminal_mut()?.take_notification()
    }

    #[must_use]
    pub fn rename(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }

        trimmed.clone_into(&mut self.title);
        self.has_custom_name = true;
        true
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.write_input(bytes);
        }
    }

    pub fn request_shutdown(&mut self) {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.request_shutdown(),
            PanelContent::Editor(editor) => editor.save_if_dirty(),
            PanelContent::GitChanges(_) | PanelContent::Usage(_) => {}
        }
    }

    #[must_use]
    pub fn wait_for_shutdown(&mut self, timeout: Duration) -> bool {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.wait_for_shutdown(timeout),
            PanelContent::Editor(editor) => {
                editor.save_if_dirty();
                true
            }
            PanelContent::GitChanges(_) | PanelContent::Usage(_) => true,
        }
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.shutdown_with_timeout(timeout),
            PanelContent::Editor(editor) => {
                editor.save_if_dirty();
                true
            }
            PanelContent::GitChanges(_) | PanelContent::Usage(_) => true,
        }
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.layout.position = position;
    }

    pub fn resize_layout(&mut self, size: [f32; 2]) {
        self.layout.size = size;
    }

    /// Restart the terminal process while keeping the same panel identity,
    /// layout, and session binding.  For agent panels (Codex / Claude) this
    /// resumes the existing session so no work is lost.
    ///
    /// # Errors
    ///
    /// Returns an error if the new terminal cannot be spawned.
    pub fn restart(&mut self) -> Result<()> {
        if let PanelContent::GitChanges(_) = &self.content {
            return Ok(());
        }

        if let PanelContent::Usage(_) = &self.content {
            self.content = PanelContent::Usage(UsageDashboard::new());
            return Ok(());
        }

        let Some(terminal) = self.content.terminal_mut() else {
            // Editor panels don't restart — just reload from disk if file-backed.
            if let Some(editor) = self.content.editor_mut()
                && let Some(path) = editor.file_path.clone()
                && path.exists()
            {
                *editor = MarkdownEditor::open(path)?;
            }
            return Ok(());
        };

        let rows = terminal.rows();
        let cols = terminal.cols();

        // Graceful shutdown of the old terminal.
        let _ = terminal.shutdown_with_timeout(Duration::from_secs(2));

        let should_resume = self.kind.is_agent() && self.session_binding.is_some();
        let (program, launch_args) = resolve_launch_command(
            self.launch_command.clone(),
            self.launch_args.clone(),
            self.kind,
            &self.resume,
            self.session_binding.as_ref(),
            should_resume,
        );

        if self.kind.is_agent() {
            tracing::info!(
                panel_id = self.id.0,
                kind = ?self.kind,
                resume = ?self.resume,
                session_id = self.session_binding.as_ref().map(|b| b.session_id.as_str()),
                should_resume,
                cwd = self.launch_cwd.as_ref().map(|p| p.display().to_string()).as_deref(),
                cmd = %format!("{program} {}", launch_args.join(" ")),
                "restarting agent panel"
            );
        }

        let env = agent_env(self.kind);
        self.content = PanelContent::Terminal(Terminal::spawn(TerminalSpawnOptions {
            program,
            args: launch_args,
            cwd: self.launch_cwd.clone(),
            rows,
            cols,
            cell_width: DEFAULT_CELL_WIDTH,
            cell_height: DEFAULT_CELL_HEIGHT,
            scrollback_limit: scrollback_limit_for_kind(self.kind),
            window_id: self.id.0,
            replay_bytes: Vec::new(),
            env,
        })?);

        self.launched_at_millis = current_unix_millis();
        tracing::info!("restarted panel '{}' (id={})", self.title, self.id.0);
        Ok(())
    }

    pub fn set_session_binding(&mut self, session_binding: Option<AgentSessionBinding>) {
        self.session_binding = session_binding;
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.scroll_scrollback_by(delta);
        }
    }

    pub fn set_scrollback(&mut self, scrollback: usize) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.set_scrollback(scrollback);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.resize(rows, cols, cell_width, cell_height);
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.set_focused(focused);
        }
    }

    /// Check if this panel's terminal output suggests it needs user attention.
    ///
    /// Suppressed for the first 10 seconds after launch to avoid false positives
    /// from initial prompt rendering on startup/restore.
    #[must_use]
    pub fn detect_attention(&self) -> Option<&'static str> {
        if !matches!(self.kind, PanelKind::Codex | PanelKind::Claude) {
            return None;
        }
        let age_ms = current_unix_millis().saturating_sub(self.launched_at_millis);
        if age_ms < 10_000 {
            return None;
        }
        let terminal = self.content.terminal()?;
        let text = terminal.last_lines_text(3);
        if text.is_empty() {
            return None;
        }
        for line in text.lines().rev() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("Allow")
                || trimmed.starts_with("Do you want")
                || trimmed.ends_with("[y/N]")
                || trimmed.ends_with("[Y/n]")
                || trimmed.ends_with("(y/n)")
            {
                return Some("Waiting for approval");
            }
            if trimmed.ends_with('?') && trimmed.len() > 2 {
                return Some("Waiting for input");
            }
            if trimmed.starts_with('>') || trimmed.starts_with("❯") {
                return Some("Ready for input");
            }
        }
        None
    }
}

fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    resume: &PanelResume,
    session_binding: Option<&AgentSessionBinding>,
    should_resume_binding: bool,
) -> (String, Vec<String>) {
    match kind {
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => (String::new(), Vec::new()),
        PanelKind::Shell => (command.unwrap_or_else(default_shell), args),
        PanelKind::Command => {
            if let Some(program) = command {
                (program, args)
            } else {
                (default_shell(), args)
            }
        }
        PanelKind::Codex => {
            let program = command.unwrap_or_else(|| "codex".to_string());
            // Global flags (e.g. --no-alt-screen) must come before the
            // resume subcommand.
            let mut launch_args = args;
            if should_resume_binding {
                if let Some(binding) = session_binding {
                    launch_args.extend(["resume".to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend(["resume".to_string(), session_id.clone()]);
            }
            wrap_in_login_shell(program, launch_args)
        }
        PanelKind::Claude => {
            let program = command.unwrap_or_else(|| "claude".to_string());
            let mut launch_args = Vec::new();
            if let Some(plugin_path) = horizon_claude_plugin_dir() {
                launch_args.extend(["--plugin-dir".to_string(), plugin_path]);
            }
            if let Some(binding) = session_binding {
                if should_resume_binding {
                    launch_args.extend(["--resume".to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend(["--resume".to_string(), session_id.clone()]);
            } else {
                // Force a new session so multiple Claude panels in the
                // same CWD don't share the most-recent conversation.
                // This UUID is only used for launch isolation — the real
                // session binding is discovered from the catalog later.
                launch_args.extend(["--session-id".to_string(), Uuid::new_v4().to_string()]);
            }
            launch_args.extend(args);
            wrap_in_login_shell(program, launch_args)
        }
    }
}

pub fn current_unix_millis() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(now).unwrap_or(i64::MAX)
}

fn prepare_transcript_restore(
    id: PanelId,
    kind: PanelKind,
    transcript_root: Option<PathBuf>,
    local_id: &str,
) -> (Option<PanelTranscript>, Vec<u8>) {
    let mut transcript = PanelTranscript::for_panel(kind, transcript_root, local_id);
    let replay_bytes = if let Some(active_transcript) = transcript.as_ref() {
        match active_transcript.prepare_replay_bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    panel_id = id.0,
                    kind = ?kind,
                    "failed to prepare persisted transcript, starting fresh shell: {error}"
                );
                transcript = None;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    (transcript, replay_bytes)
}

fn resolve_session_binding(
    kind: PanelKind,
    resume: &PanelResume,
    mut session_binding: Option<AgentSessionBinding>,
    cwd: Option<&str>,
    label: Option<&str>,
) -> (Option<AgentSessionBinding>, bool) {
    let had_existing_session_binding = session_binding.is_some();
    if session_binding.is_none() {
        session_binding = match (resume, kind) {
            (PanelResume::Session { session_id }, PanelKind::Codex | PanelKind::Claude) => {
                Some(AgentSessionBinding::new(
                    kind,
                    session_id.clone(),
                    cwd.map(str::to_string),
                    label.map(str::to_string),
                    None,
                ))
            }
            (PanelResume::Fresh, PanelKind::Claude) => {
                // Don't generate a synthetic session binding — Claude Code
                // only creates the session file after the first user message,
                // so a pre-assigned UUID would not match any on-disk session.
                // The binding is captured later once the session exists.
                None
            }
            _ => None,
        };
    }

    let should_resume_binding = match kind {
        PanelKind::Claude => {
            session_binding.is_some()
                && (had_existing_session_binding || matches!(resume, PanelResume::Last | PanelResume::Session { .. }))
        }
        PanelKind::Codex
        | PanelKind::Shell
        | PanelKind::Command
        | PanelKind::Editor
        | PanelKind::GitChanges
        | PanelKind::Usage => session_binding.is_some() || matches!(resume, PanelResume::Session { .. }),
    };

    (session_binding, should_resume_binding)
}

/// Wrap a command in an interactive shell (`-ic`) so that the user's
/// full PATH (nvm, cargo, etc. from `~/.bashrc`) is available.
fn wrap_in_login_shell(program: String, args: Vec<String>) -> (String, Vec<String>) {
    let shell = default_shell();
    let mut cmd_parts = vec![program];
    cmd_parts.extend(args);
    let joined = cmd_parts.iter().map(|a| shell_escape(a)).collect::<Vec<_>>().join(" ");
    (shell, vec!["-ic".to_string(), joined])
}

fn shell_escape(s: &str) -> String {
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\' || c == '$') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

const fn platform_default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| platform_default_shell().to_string())
}

fn agent_env(kind: PanelKind) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if kind.is_agent() {
        env.insert("HORIZON".to_string(), "1".to_string());
    }
    env
}

fn horizon_claude_plugin_dir() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home)
        .join(".config")
        .join("horizon")
        .join("plugins")
        .join("claude-code");
    if path.is_dir() {
        Some(path.display().to_string())
    } else {
        None
    }
}

fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    match kind {
        PanelKind::Codex | PanelKind::Claude => AGENT_PANEL_SCROLLBACK_LIMIT,
        PanelKind::Shell | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => 0,
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_state::AgentSessionBinding;

    use super::{
        AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SCROLLBACK_LIMIT, PanelKind, PanelResume, platform_default_shell,
        resolve_launch_command, scrollback_limit_for_kind,
    };

    #[test]
    fn codex_without_exact_binding_starts_fresh() {
        let (_program, args) =
            resolve_launch_command(None, Vec::new(), PanelKind::Codex, &PanelResume::Last, None, false);

        // Codex is launched via login shell without resume when no exact session is bound.
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("codex"));
        assert!(!args[1].contains("resume"));
    }

    #[test]
    fn claude_session_resume_uses_resume_flag() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            PanelKind::Claude,
            &PanelResume::Session {
                session_id: "session-42".to_string(),
            },
            Some(&binding),
            true,
        );

        // Claude is launched via login shell: shell -lc "claude --resume session-42"
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--resume session-42"));
    }

    #[test]
    fn claude_fresh_without_binding_uses_ephemeral_session_id() {
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--dangerously-skip-permissions".to_string()],
            PanelKind::Claude,
            &PanelResume::Fresh,
            None,
            false,
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--dangerously-skip-permissions"));
        assert!(
            args[1].contains("--session-id"),
            "fresh Claude panels must get an ephemeral --session-id"
        );
        assert!(!args[1].contains("--resume"));
    }

    #[test]
    fn restored_claude_fresh_binding_uses_resume_flag() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--dangerously-skip-permissions".to_string()],
            PanelKind::Claude,
            &PanelResume::Fresh,
            Some(&binding),
            true,
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--resume session-42"));
        assert!(!args[1].contains("--session-id session-42"));
    }

    #[test]
    fn codex_global_flags_come_before_exact_resume_subcommand() {
        let binding = AgentSessionBinding::new(PanelKind::Codex, "thread-7".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--no-alt-screen".to_string()],
            PanelKind::Codex,
            &PanelResume::Last,
            Some(&binding),
            true,
        );

        let cmd = &args[1];
        let flag_pos = cmd.find("--no-alt-screen").expect("flag present");
        let resume_pos = cmd.find("resume thread-7").expect("resume present");
        assert!(
            flag_pos < resume_pos,
            "global flags must precede resume subcommand: {cmd}"
        );
    }

    #[test]
    fn explicit_command_wins_over_kind_defaults() {
        let (_program, args) = resolve_launch_command(
            Some("python".to_string()),
            vec!["-m".to_string(), "http.server".to_string()],
            PanelKind::Codex,
            &PanelResume::Last,
            None,
            false,
        );

        // Explicit command is still wrapped in login shell for agent kinds
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("python"));
        assert!(args[1].contains("-m"));
        assert!(args[1].contains("http.server"));
    }

    #[test]
    fn agent_panels_get_deeper_scrollback() {
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Shell),
            DEFAULT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Codex),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Claude),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
    }

    #[test]
    fn platform_default_shell_matches_target() {
        let expected = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };

        assert_eq!(platform_default_shell(), expected);
    }
}
