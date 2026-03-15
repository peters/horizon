use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::Result;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, new_local_id, new_session_binding};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::workspace::WorkspaceId;

const DEFAULT_CELL_WIDTH: u16 = 8;
const DEFAULT_CELL_HEIGHT: u16 = 17;

pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];
const DEFAULT_PANEL_SCROLLBACK_LIMIT: usize = 8_000;
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
}

impl PanelKind {
    #[must_use]
    pub fn is_agent(self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
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
    pub terminal: Terminal,
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
    /// Spawn a new PTY-backed terminal panel.
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
            mut session_binding,
            template,
        } = opts;

        // Save original launch params for persistence before they're
        // transformed by resolve_launch_command.
        let saved_command = command.clone();
        let saved_args = args.clone();
        let saved_cwd = cwd.clone();
        let saved_cwd_string = saved_cwd.as_ref().map(|path| path.display().to_string());
        let had_existing_session_binding = session_binding.is_some();
        if session_binding.is_none() {
            session_binding = match (&resume, kind) {
                (PanelResume::Session { session_id }, PanelKind::Codex | PanelKind::Claude) => Some(
                    AgentSessionBinding::new(kind, session_id.clone(), saved_cwd_string.clone(), name.clone(), None),
                ),
                (PanelResume::Fresh, PanelKind::Claude) => {
                    new_session_binding(kind, saved_cwd_string.clone(), name.clone())
                }
                _ => None,
            };
        }

        let should_resume_binding = match kind {
            PanelKind::Claude => {
                session_binding.is_some()
                    && (had_existing_session_binding
                        || matches!(resume, PanelResume::Last | PanelResume::Session { .. }))
            }
            PanelKind::Codex | PanelKind::Shell | PanelKind::Command => {
                session_binding.is_some() || matches!(resume, PanelResume::Session { .. })
            }
        };

        let (program, launch_args) = resolve_launch_command(
            command,
            args,
            kind,
            &resume,
            session_binding.as_ref(),
            should_resume_binding,
        );
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
        })?;

        tracing::info!("created panel '{}' (id={})", title, id.0);

        Ok(Self {
            id,
            local_id: local_id.unwrap_or_else(new_local_id),
            title,
            kind,
            resume,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            terminal,
            session_binding,
            template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            launch_command: saved_command,
            launch_args: saved_args,
            launch_cwd: saved_cwd,
        })
    }

    pub fn process_output(&mut self) {
        self.terminal.process_events();

        if !self.has_custom_name {
            let title = self.terminal.title();
            if !title.is_empty() {
                self.title = title.to_string();
            }
        }
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
        self.terminal.write_input(bytes);
    }

    pub fn request_shutdown(&mut self) {
        self.terminal.request_shutdown();
    }

    #[must_use]
    pub fn wait_for_shutdown(&mut self, timeout: Duration) -> bool {
        self.terminal.wait_for_shutdown(timeout)
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.terminal.shutdown_with_timeout(timeout)
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.layout.position = position;
    }

    pub fn resize_layout(&mut self, size: [f32; 2]) {
        self.layout.size = size;
    }

    pub fn set_session_binding(&mut self, session_binding: Option<AgentSessionBinding>) {
        self.session_binding = session_binding;
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        self.terminal.scroll_scrollback_by(delta);
    }

    pub fn set_scrollback(&mut self, scrollback: usize) {
        self.terminal.set_scrollback(scrollback);
    }

    pub fn resize(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        self.terminal.resize(rows, cols, cell_width, cell_height);
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.terminal.set_focused(focused);
    }

    /// Check if this panel's terminal output suggests it needs user attention.
    #[must_use]
    pub fn detect_attention(&self) -> Option<&'static str> {
        if !matches!(self.kind, PanelKind::Codex | PanelKind::Claude) {
            return None;
        }
        let text = self.terminal.last_lines_text(3);
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
            if let Some(binding) = session_binding {
                if should_resume_binding {
                    launch_args.extend(["--resume".to_string(), binding.session_id.clone()]);
                } else {
                    launch_args.extend(["--session-id".to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend(["--resume".to_string(), session_id.clone()]);
            }
            launch_args.extend(args);
            wrap_in_login_shell(program, launch_args)
        }
    }
}

fn current_unix_millis() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(now).unwrap_or(i64::MAX)
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

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    match kind {
        PanelKind::Codex | PanelKind::Claude => AGENT_PANEL_SCROLLBACK_LIMIT,
        PanelKind::Shell | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime_state::AgentSessionBinding;

    use super::{
        AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SCROLLBACK_LIMIT, PanelKind, PanelResume, resolve_launch_command,
        scrollback_limit_for_kind,
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
    fn claude_fresh_binding_uses_session_id_flag() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--dangerously-skip-permissions".to_string()],
            PanelKind::Claude,
            &PanelResume::Fresh,
            Some(&binding),
            false,
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--session-id session-42"));
        assert!(!args[1].contains("--resume session-42"));
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
}
