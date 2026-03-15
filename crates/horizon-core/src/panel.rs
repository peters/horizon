use std::path::PathBuf;

use serde::Deserialize;

use crate::error::Result;
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::workspace::WorkspaceId;

const DEFAULT_CELL_WIDTH: u16 = 8;
const DEFAULT_CELL_HEIGHT: u16 = 17;

pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];
const DEFAULT_PANEL_SCROLLBACK_LIMIT: usize = 8_000;
const AGENT_PANEL_SCROLLBACK_LIMIT: usize = 24_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Shell,
    Codex,
    Claude,
    Command,
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
        }
    }
}

pub struct Panel {
    pub id: PanelId,
    pub title: String,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub layout: PanelLayout,
    pub workspace_id: WorkspaceId,
    pub terminal: Terminal,
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
        } = opts;

        // Save original launch params for persistence before they're
        // transformed by resolve_launch_command.
        let saved_command = command.clone();
        let saved_args = args.clone();
        let saved_cwd = cwd.clone();

        let (program, launch_args) = resolve_launch_command(command, args, kind, &resume);
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
            title,
            kind,
            resume,
            layout: PanelLayout {
                position: position.unwrap_or_default(),
                size: size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id,
            terminal,
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

    pub fn write_input(&mut self, bytes: &[u8]) {
        self.terminal.write_input(bytes);
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.layout.position = position;
    }

    pub fn resize_layout(&mut self, size: [f32; 2]) {
        self.layout.size = size;
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
            match resume {
                PanelResume::Fresh => {}
                PanelResume::Last => launch_args.extend(["resume".to_string(), "--last".to_string()]),
                PanelResume::Session { session_id } => {
                    launch_args.extend(["resume".to_string(), session_id.clone()]);
                }
            }
            wrap_in_login_shell(program, launch_args)
        }
        PanelKind::Claude => {
            let program = command.unwrap_or_else(|| "claude".to_string());
            let mut launch_args = match resume {
                PanelResume::Fresh => Vec::new(),
                PanelResume::Last => vec!["--continue".to_string()],
                PanelResume::Session { session_id } => vec!["--resume".to_string(), session_id.clone()],
            };
            launch_args.extend(args);
            wrap_in_login_shell(program, launch_args)
        }
    }
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
    use super::{
        AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SCROLLBACK_LIMIT, PanelKind, PanelResume, resolve_launch_command,
        scrollback_limit_for_kind,
    };

    #[test]
    fn codex_last_resume_uses_resume_subcommand() {
        let (_program, args) = resolve_launch_command(None, Vec::new(), PanelKind::Codex, &PanelResume::Last);

        // Codex is launched via login shell: shell -lc "codex resume --last"
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("codex"));
        assert!(args[1].contains("resume --last"));
    }

    #[test]
    fn claude_session_resume_uses_resume_flag() {
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            PanelKind::Claude,
            &PanelResume::Session {
                session_id: "session-42".to_string(),
            },
        );

        // Claude is launched via login shell: shell -lc "claude --resume session-42"
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--resume session-42"));
    }

    #[test]
    fn codex_global_flags_come_before_resume_subcommand() {
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--no-alt-screen".to_string()],
            PanelKind::Codex,
            &PanelResume::Last,
        );

        let cmd = &args[1];
        let flag_pos = cmd.find("--no-alt-screen").expect("flag present");
        let resume_pos = cmd.find("resume --last").expect("resume present");
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
