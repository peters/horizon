mod spawn;

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::git_changes::DiffViewer;
use crate::remote_hosts::RemoteHostsPanel;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef};
use crate::ssh::{SshConnection, SshConnectionStatus};
use crate::terminal::{AgentNotification, Terminal, TerminalSpawnOptions};
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;

pub use self::spawn::current_unix_millis;
#[cfg(test)]
use self::spawn::platform_default_shell;
use self::spawn::{agent_env, kitty_keyboard_for_kind, resolve_launch_command, scrollback_limit_for_kind, spawn_panel};

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
    Ssh,
    Codex,
    Claude,
    Command,
    Editor,
    GitChanges,
    RemoteHosts,
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
            Self::Ssh => "SSH",
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Command => "Command",
            Self::Editor => "Editor",
            Self::GitChanges => "Git Changes",
            Self::RemoteHosts => "Remote Hosts",
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
    pub ssh_connection: Option<SshConnection>,
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
            ssh_connection: None,
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
    pub terminal_title: String,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub layout: PanelLayout,
    pub workspace_id: WorkspaceId,
    pub content: PanelContent,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    pub launched_at_millis: i64,
    has_custom_name: bool,
    /// Set by `process_output` each frame; read by attention detection to skip
    /// the expensive `last_lines_text` scan for panels without new content.
    pub(crate) had_recent_output: bool,
    /// Original launch command (for persistence).
    pub launch_command: Option<String>,
    /// Original launch args (for persistence).
    pub launch_args: Vec<String>,
    /// Working directory (for persistence).
    pub launch_cwd: Option<PathBuf>,
    /// Structured SSH connection metadata, when this is an SSH panel.
    pub ssh_connection: Option<SshConnection>,
    /// UI-facing SSH connection status for panels that remain visible after exit.
    pub ssh_status: Option<SshConnectionStatus>,
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

    #[must_use]
    pub fn ssh_status(&self) -> Option<SshConnectionStatus> {
        self.ssh_status
    }

    #[must_use]
    pub const fn had_recent_output(&self) -> bool {
        self.had_recent_output
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
    pub fn git_changes(&self) -> Option<&DiffViewer> {
        self.content.git_changes()
    }

    /// Mutable accessor for the git changes content.
    pub fn git_changes_mut(&mut self) -> Option<&mut DiffViewer> {
        self.content.git_changes_mut()
    }

    /// Convenience accessor for the remote hosts content (if this panel holds it).
    #[must_use]
    pub fn remote_hosts(&self) -> Option<&RemoteHostsPanel> {
        self.content.remote_hosts()
    }

    /// Mutable accessor for the remote hosts content.
    pub fn remote_hosts_mut(&mut self) -> Option<&mut RemoteHostsPanel> {
        self.content.remote_hosts_mut()
    }
}

impl Panel {
    /// Spawn a new panel — either a PTY-backed terminal or a markdown editor.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal runtime cannot be created.
    pub fn spawn(id: PanelId, workspace_id: WorkspaceId, opts: PanelOptions) -> Result<Self> {
        spawn_panel(id, workspace_id, opts)
    }

    /// Drain pending terminal events. Returns `true` if any output was processed.
    #[profiling::function]
    pub fn process_output(&mut self) -> bool {
        let Some(terminal) = self.content.terminal_mut() else {
            self.had_recent_output = false;
            return false;
        };
        let had_output = terminal.process_events();
        self.had_recent_output = had_output;

        if had_output {
            self.terminal_title = terminal.title().to_string();
        }
        if self.kind == PanelKind::Ssh {
            if terminal.child_exited() {
                self.ssh_status = Some(SshConnectionStatus::Disconnected);
            } else if had_output && matches!(self.ssh_status, Some(SshConnectionStatus::Connecting)) {
                self.ssh_status = Some(SshConnectionStatus::Connected);
            }
        }
        had_output
    }

    #[must_use]
    pub fn display_title(&self) -> Cow<'_, str> {
        if self.kind == PanelKind::Ssh {
            let base_title = if self.has_custom_name {
                self.ssh_connection
                    .as_ref()
                    .map(SshConnection::display_label)
                    .filter(|host_label| !self.title.contains(host_label))
                    .map_or_else(
                        || self.title.clone(),
                        |host_label| format!("{} ({host_label})", self.title),
                    )
            } else {
                self.title.clone()
            };

            if self.terminal_title.is_empty() || self.terminal_title == self.title || self.terminal_title == base_title
            {
                return Cow::Owned(base_title);
            }

            return Cow::Owned(format!("{base_title} — {}", self.terminal_title));
        }

        if self.has_custom_name {
            if self.terminal_title.is_empty() || self.terminal_title == self.title {
                Cow::Borrowed(&self.title)
            } else {
                Cow::Owned(format!("{} — {}", self.title, self.terminal_title))
            }
        } else if self.terminal_title.is_empty() {
            Cow::Borrowed(&self.title)
        } else {
            Cow::Borrowed(&self.terminal_title)
        }
    }

    #[must_use]
    pub fn child_exited(&self) -> bool {
        self.content.terminal().is_some_and(Terminal::child_exited)
    }

    #[must_use]
    pub fn should_close_after_exit(&self) -> bool {
        !matches!(self.kind, PanelKind::Ssh)
    }

    /// Returns `true` if the terminal bell has fired since the last call.
    pub fn take_bell(&mut self) -> bool {
        self.content.terminal_mut().is_some_and(Terminal::take_bell)
    }

    pub fn take_notification(&mut self) -> Option<AgentNotification> {
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
            PanelContent::GitChanges(_) | PanelContent::RemoteHosts(_) | PanelContent::Usage(_) => {}
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
            PanelContent::GitChanges(_) | PanelContent::RemoteHosts(_) | PanelContent::Usage(_) => true,
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
            PanelContent::GitChanges(_) | PanelContent::RemoteHosts(_) | PanelContent::Usage(_) => true,
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

        if let PanelContent::RemoteHosts(_) = &self.content {
            self.content = PanelContent::RemoteHosts(RemoteHostsPanel::new());
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
            self.ssh_connection.clone(),
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
            kitty_keyboard: kitty_keyboard_for_kind(self.kind),
        })?);

        self.launched_at_millis = current_unix_millis();
        self.ssh_status = if self.kind == PanelKind::Ssh {
            Some(SshConnectionStatus::Connecting)
        } else {
            None
        };
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

    pub fn resize_immediately(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.resize_immediately(rows, cols, cell_width, cell_height);
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

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PANEL_SCROLLBACK_LIMIT, AgentSessionBinding, DEFAULT_PANEL_SCROLLBACK_LIMIT, Panel, PanelContent,
        PanelId, PanelKind, PanelLayout, PanelResume, UsageDashboard, WorkspaceId, kitty_keyboard_for_kind,
        platform_default_shell, resolve_launch_command, scrollback_limit_for_kind,
    };
    use crate::ssh::SshConnection;

    fn test_panel(title: &str, terminal_title: &str, has_custom_name: bool) -> Panel {
        Panel {
            id: PanelId(1),
            local_id: "panel-1".to_string(),
            title: title.to_string(),
            terminal_title: terminal_title.to_string(),
            kind: PanelKind::Usage,
            resume: PanelResume::Fresh,
            layout: PanelLayout::default(),
            workspace_id: WorkspaceId(1),
            content: PanelContent::Usage(UsageDashboard::new()),
            session_binding: None,
            template: None,
            launched_at_millis: 0,
            has_custom_name,
            had_recent_output: false,
            launch_command: None,
            launch_args: Vec::new(),
            launch_cwd: None,
            ssh_connection: None,
            ssh_status: None,
        }
    }

    #[test]
    fn display_title_uses_runtime_title_for_unnamed_panels() {
        let panel = test_panel("Terminal 1", "Build running", false);

        assert_eq!(panel.display_title(), "Build running");
    }

    #[test]
    fn display_title_appends_runtime_title_for_custom_named_panels() {
        let panel = test_panel("Backend", "Build running", true);

        assert_eq!(panel.display_title(), "Backend — Build running");
    }

    #[test]
    fn display_title_omits_duplicate_runtime_title_for_custom_named_panels() {
        let panel = test_panel("Backend", "Backend", true);

        assert_eq!(panel.display_title(), "Backend");
    }

    #[test]
    fn display_title_keeps_ssh_host_visible_for_custom_named_panels() {
        let mut panel = test_panel("Prod API", "Connected", true);
        panel.kind = PanelKind::Ssh;
        panel.ssh_connection = Some(SshConnection {
            host: "prod-api".to_string(),
            user: Some("deploy".to_string()),
            ..SshConnection::default()
        });

        assert_eq!(panel.display_title(), "Prod API (deploy@prod-api) — Connected");
    }

    #[test]
    fn ssh_panels_do_not_auto_close_after_exit() {
        let mut panel = test_panel("SSH: prod", "", false);
        panel.kind = PanelKind::Ssh;

        assert!(!panel.should_close_after_exit());
    }

    #[test]
    fn codex_without_exact_binding_starts_fresh() {
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Codex,
            &PanelResume::Last,
            None,
            false,
        );

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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            scrollback_limit_for_kind(PanelKind::Ssh),
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
    fn codex_panels_disable_kitty_keyboard_protocol() {
        assert!(!kitty_keyboard_for_kind(PanelKind::Codex));
        assert!(kitty_keyboard_for_kind(PanelKind::Claude));
        assert!(kitty_keyboard_for_kind(PanelKind::Shell));
        assert!(kitty_keyboard_for_kind(PanelKind::Ssh));
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
