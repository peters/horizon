use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::terminal::Terminal;
use crate::workspace::WorkspaceId;

const MAX_OUTPUT_CHUNKS_PER_FRAME: usize = 24;
const MAX_OUTPUT_BYTES_PER_FRAME: usize = 128 * 1024;
const PTY_RESIZE_INTERVAL: Duration = Duration::from_millis(40);
pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Shell,
    Codex,
    Claude,
    Command,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
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
    pub auto_resize_pty: bool,
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
            auto_resize_pty: true,
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
    pub workspace_id: Option<WorkspaceId>,
    pub auto_resize_pty: bool,
    pub terminal: Terminal,
    has_custom_name: bool,
    writer: Box<dyn Write + Send>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    pending_pty_resize: Option<(u16, u16)>,
    last_pty_resize_at: Instant,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Panel {
    /// Spawn a new PTY-backed terminal panel.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be created, the command cannot be
    /// spawned, or the PTY reader/writer handles cannot be acquired.
    pub fn spawn(id: PanelId, opts: PanelOptions) -> Result<Self> {
        let pty_system = native_pty_system();
        let PanelOptions {
            name,
            command,
            args,
            cwd,
            rows,
            cols,
            auto_resize_pty,
            kind,
            resume,
            position,
            size,
        } = opts;
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.to_string()))?;

        let (program, launch_args) = resolve_launch_command(command, args, kind, &resume);
        let mut cmd = CommandBuilder::new(&program);
        for arg in &launch_args {
            cmd.arg(arg);
        }
        if let Some(cwd) = &cwd {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| Error::Pty(e.to_string()))?;

        let reader = pair.master.try_clone_reader().map_err(|e| Error::Pty(format!("{e}")))?;
        let writer = pair.master.take_writer().map_err(|e| Error::Pty(format!("{e}")))?;

        let (tx, rx) = mpsc::channel();

        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            tracing::debug!("PTY reader thread exited");
        });

        let has_custom_name = name.is_some();
        let title = name.unwrap_or_else(|| format!("Terminal {}", id.0));
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
            workspace_id: None,
            auto_resize_pty,
            terminal: Terminal::new(rows, cols),
            has_custom_name,
            writer,
            output_rx: rx,
            master: pair.master,
            pending_pty_resize: None,
            last_pty_resize_at: Instant::now()
                .checked_sub(PTY_RESIZE_INTERVAL)
                .unwrap_or_else(Instant::now),
            _child: child,
        })
    }

    /// Drain pending PTY output and feed to the terminal emulator.
    pub fn process_output(&mut self) {
        self.flush_pending_pty_resize();

        let mut processed_bytes = 0_usize;
        let mut processed_chunks = 0_usize;
        while processed_chunks < MAX_OUTPUT_CHUNKS_PER_FRAME && processed_bytes < MAX_OUTPUT_BYTES_PER_FRAME {
            let Ok(bytes) = self.output_rx.try_recv() else {
                break;
            };

            processed_bytes += bytes.len();
            self.terminal.process(&bytes);
            processed_chunks += 1;
        }

        self.flush_pending_pty_resize();

        // Only override title from terminal escape sequences if no custom name was set
        if !self.has_custom_name {
            let title = self.terminal.title();
            if !title.is_empty() {
                self.title = title.to_string();
            }
        }
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.layout.position = position;
    }

    pub fn resize_layout(&mut self, size: [f32; 2]) {
        self.layout.size = size;
    }

    pub fn set_auto_resize_pty(&mut self, enabled: bool) {
        self.auto_resize_pty = enabled;
    }

    pub fn adjust_pty_size(&mut self, row_delta: i16, col_delta: i16) {
        self.auto_resize_pty = false;
        let rows = adjust_dimension(self.terminal.rows(), row_delta);
        let cols = adjust_dimension(self.terminal.cols(), col_delta);
        self.resize(rows, cols);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.terminal.rows() && cols == self.terminal.cols() {
            return;
        }

        self.terminal.resize(rows, cols);
        self.pending_pty_resize = Some((rows, cols));
        self.flush_pending_pty_resize();
    }

    fn flush_pending_pty_resize(&mut self) {
        if self.last_pty_resize_at.elapsed() < PTY_RESIZE_INTERVAL {
            return;
        }

        let Some((rows, cols)) = self.pending_pty_resize.take() else {
            return;
        };

        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.last_pty_resize_at = Instant::now();
    }
}

fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    resume: &PanelResume,
) -> (String, Vec<String>) {
    if let Some(program) = command {
        return (program, args);
    }

    match kind {
        PanelKind::Shell | PanelKind::Command => (default_shell(), args),
        PanelKind::Codex => {
            let mut launch_args = match resume {
                PanelResume::Fresh => Vec::new(),
                PanelResume::Last => vec!["resume".to_string(), "--last".to_string()],
                PanelResume::Session { session_id } => vec!["resume".to_string(), session_id.clone()],
            };
            launch_args.extend(args);
            ("codex".to_string(), launch_args)
        }
        PanelKind::Claude => {
            let mut launch_args = match resume {
                PanelResume::Fresh => Vec::new(),
                PanelResume::Last => vec!["--continue".to_string()],
                PanelResume::Session { session_id } => vec!["--resume".to_string(), session_id.clone()],
            };
            launch_args.extend(args);
            ("claude".to_string(), launch_args)
        }
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
}

fn adjust_dimension(current: u16, delta: i16) -> u16 {
    let adjusted = i32::from(current) + i32::from(delta);
    adjusted.clamp(1, i32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::{PanelKind, PanelResume, resolve_launch_command};

    #[test]
    fn codex_last_resume_uses_resume_subcommand() {
        let (program, args) = resolve_launch_command(None, Vec::new(), PanelKind::Codex, &PanelResume::Last);

        assert_eq!(program, "codex");
        assert_eq!(args, vec!["resume", "--last"]);
    }

    #[test]
    fn claude_session_resume_uses_resume_flag() {
        let (program, args) = resolve_launch_command(
            None,
            Vec::new(),
            PanelKind::Claude,
            &PanelResume::Session {
                session_id: "session-42".to_string(),
            },
        );

        assert_eq!(program, "claude");
        assert_eq!(args, vec!["--resume", "session-42"]);
    }

    #[test]
    fn explicit_command_wins_over_kind_defaults() {
        let (program, args) = resolve_launch_command(
            Some("python".to_string()),
            vec!["-m".to_string(), "http.server".to_string()],
            PanelKind::Codex,
            &PanelResume::Last,
        );

        assert_eq!(program, "python");
        assert_eq!(args, vec!["-m", "http.server"]);
    }
}
