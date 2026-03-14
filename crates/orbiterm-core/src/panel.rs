use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::error::{Error, Result};
use crate::terminal::Terminal;

const MAX_OUTPUT_CHUNKS_PER_FRAME: usize = 24;
const MAX_OUTPUT_BYTES_PER_FRAME: usize = 128 * 1024;
const PTY_RESIZE_INTERVAL: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(pub u64);

pub struct PanelOptions {
    pub name: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
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
        }
    }
}

pub struct Panel {
    pub id: PanelId,
    pub title: String,
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
        let pair = pty_system
            .openpty(PtySize {
                rows: opts.rows,
                cols: opts.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.to_string()))?;

        let program = opts
            .command
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()));
        let mut cmd = CommandBuilder::new(&program);
        for arg in &opts.args {
            cmd.arg(arg);
        }
        if let Some(cwd) = &opts.cwd {
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

        let has_custom_name = opts.name.is_some();
        let title = opts.name.unwrap_or_else(|| format!("Terminal {}", id.0));
        tracing::info!("created panel '{}' (id={})", title, id.0);

        Ok(Self {
            id,
            title,
            terminal: Terminal::new(opts.rows, opts.cols),
            has_custom_name,
            writer,
            output_rx: rx,
            master: pair.master,
            pending_pty_resize: None,
            last_pty_resize_at: Instant::now() - PTY_RESIZE_INTERVAL,
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
