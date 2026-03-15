use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, RenderableContent, Term, TermDamage, TermMode};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
use alacritty_terminal::vte::ansi::Rgb;

use crate::error::{Error, Result};

#[cfg(not(windows))]
type TerminalPty = tty::Pty;
#[cfg(windows)]
type TerminalPty = tty::Pty;

type TerminalEventLoop = EventLoop<TerminalPty, TerminalEventProxy>;
type TerminalEventLoopState = State;

pub struct TerminalSpawnOptions {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    pub scrollback_limit: usize,
    pub window_id: u64,
}

#[derive(Clone)]
struct TerminalEventProxy {
    event_tx: mpsc::Sender<Event>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.event_tx.send(event);
    }
}

#[derive(Clone, Copy)]
struct TerminalDimensions {
    rows: usize,
    cols: usize,
}

impl TerminalDimensions {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: usize::from(rows.max(1)),
            cols: usize::from(cols.max(2)),
        }
    }
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,
    event_sender: EventLoopSender,
    event_rx: mpsc::Receiver<Event>,
    event_loop_handle: Option<JoinHandle<(TerminalEventLoop, TerminalEventLoopState)>>,
    rows: u16,
    cols: u16,
    cell_width: u16,
    cell_height: u16,
    scrollback_limit: usize,
    title: String,
    clipboard_contents: String,
    selection_contents: String,
    pending_pty_resize: Option<std::time::Instant>,
    pty_resized: bool,
}

impl Terminal {
    /// Spawn a terminal session backed by `alacritty_terminal`.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY or event loop cannot be created.
    pub fn spawn(options: TerminalSpawnOptions) -> Result<Self> {
        let rows = options.rows.max(1);
        let cols = options.cols.max(2);
        let scrollback_limit = options.scrollback_limit.max(1);
        let cell_width = options.cell_width.max(1);
        let cell_height = options.cell_height.max(1);
        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        };
        let dimensions = TerminalDimensions::new(rows, cols);
        let terminal_config = term::Config {
            scrolling_history: scrollback_limit,
            kitty_keyboard: true,
            ..term::Config::default()
        };
        let (event_tx, event_rx) = mpsc::channel();
        let term_proxy = TerminalEventProxy {
            event_tx: event_tx.clone(),
        };
        let event_loop_proxy = TerminalEventProxy { event_tx };

        tty::setup_env();

        let pty_options = PtyOptions {
            shell: Some(Shell::new(options.program, options.args)),
            working_directory: options.cwd,
            drain_on_exit: true,
            env: HashMap::new(),
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let term = Arc::new(FairMutex::new(Term::new(terminal_config, &dimensions, term_proxy)));
        let pty =
            tty::new(&pty_options, window_size, options.window_id).map_err(|error| Error::Pty(error.to_string()))?;
        let event_loop = EventLoop::new(term.clone(), event_loop_proxy, pty, true, false)
            .map_err(|error| Error::Pty(format!("failed to initialize terminal event loop: {error}")))?;
        let event_sender = event_loop.channel();
        let event_loop_handle = Some(event_loop.spawn());

        let mut terminal = Self {
            term,
            event_sender,
            event_rx,
            event_loop_handle,
            rows,
            cols,
            cell_width,
            cell_height,
            scrollback_limit,
            title: String::new(),
            clipboard_contents: String::new(),
            selection_contents: String::new(),
            pending_pty_resize: None,
            pty_resized: false,
        };
        terminal.process_events();
        Ok(terminal)
    }

    pub fn process_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }
        self.flush_pending_pty_resize();
    }

    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let _ = self
            .event_sender
            .send(Msg::Input(Cow::Owned(bytes.to_vec())))
            .map_err(|error| tracing::debug!("failed to forward terminal input: {error}"));
    }

    pub fn resize(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let cell_width = cell_width.max(1);
        let cell_height = cell_height.max(1);

        if rows == self.rows && cols == self.cols && cell_width == self.cell_width && cell_height == self.cell_height {
            self.flush_pending_pty_resize();
            return;
        }

        self.rows = rows;
        self.cols = cols;
        self.cell_width = cell_width;
        self.cell_height = cell_height;

        // Resize the terminal grid immediately for smooth visual feedback.
        self.term.lock().resize(TerminalDimensions::new(self.rows, self.cols));

        if self.pty_resized {
            // Debounce subsequent PTY resizes to avoid flooding the child
            // process during drag-resize.
            self.pending_pty_resize = Some(std::time::Instant::now());
        } else {
            // First resize after spawn — send immediately so the child
            // starts with the correct terminal dimensions.
            self.pty_resized = true;
            let _ = self
                .event_sender
                .send(Msg::Resize(self.window_size()))
                .map_err(|error| tracing::debug!("failed to resize terminal PTY: {error}"));
        }
    }

    fn flush_pending_pty_resize(&mut self) {
        const PTY_RESIZE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(80);

        let Some(requested_at) = self.pending_pty_resize else {
            return;
        };
        if requested_at.elapsed() < PTY_RESIZE_DEBOUNCE {
            return;
        }
        self.pending_pty_resize = None;
        let _ = self
            .event_sender
            .send(Msg::Resize(self.window_size()))
            .map_err(|error| tracing::debug!("failed to resize terminal PTY: {error}"));
    }

    #[must_use]
    pub fn scrollback(&self) -> usize {
        self.term.lock().grid().display_offset()
    }

    pub fn set_scrollback(&mut self, scrollback: usize) {
        let current = self.scrollback();
        if current == scrollback {
            return;
        }

        let current = isize::try_from(current).unwrap_or(isize::MAX);
        let target = isize::try_from(scrollback).unwrap_or(isize::MAX);
        let delta = target.saturating_sub(current);
        let delta = delta.clamp(i32::MIN as isize, i32::MAX as isize);
        #[allow(clippy::cast_possible_truncation)]
        let delta = delta as i32;

        self.term.lock().scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }

        let current = self.scrollback();
        let target = if delta.is_positive() {
            current.saturating_add(usize::try_from(delta).unwrap_or(usize::MAX))
        } else {
            current.saturating_sub(usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX))
        };
        self.set_scrollback(target);
    }

    /// Extract the last few non-empty lines visible on screen as a single
    /// string, for pattern matching (e.g. detecting agent prompts).
    #[must_use]
    pub fn last_lines_text(&self, max_lines: usize) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = usize::from(self.cols);
        let rows = usize::from(self.rows);
        let mut lines: Vec<String> = Vec::with_capacity(max_lines);
        let mut current_line = String::with_capacity(cols);
        let mut current_row: Option<usize> = None;

        for indexed in content.display_iter {
            let Ok(row) = usize::try_from(indexed.point.line.0) else {
                continue;
            };
            if row >= rows {
                continue;
            }
            if current_row != Some(row) {
                if !current_line.is_empty() {
                    lines.push(std::mem::take(&mut current_line));
                }
                current_row = Some(row);
                current_line.clear();
            }
            if indexed.cell.c != ' ' || indexed.cell.zerowidth().is_some() {
                while current_line.len() < indexed.point.column.0 {
                    current_line.push(' ');
                }
                current_line.push(indexed.cell.c);
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }

    #[must_use]
    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    #[must_use]
    pub fn history_size(&self) -> usize {
        let term = self.term.lock();
        let grid = term.grid();
        grid.total_lines().saturating_sub(grid.screen_lines())
    }

    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn set_focused(&mut self, focused: bool) {
        let mode = {
            let mut term = self.term.lock();
            if term.is_focused == focused {
                return;
            }

            term.is_focused = focused;
            *term.mode()
        };

        if mode.contains(TermMode::FOCUS_IN_OUT) {
            let sequence = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.write_input(sequence);
        }
    }

    pub fn with_renderable_content<R>(&self, render: impl FnOnce(RenderableContent<'_>) -> R) -> R {
        let term = self.term.lock();
        render(term.renderable_content())
    }

    pub fn with_damage<R>(&self, update: impl FnOnce(TermDamage<'_>) -> R) -> R {
        let mut term = self.term.lock();
        update(term.damage())
    }

    pub fn reset_damage(&self) {
        self.term.lock().reset_damage();
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Title(title) => {
                self.title = title;
            }
            Event::ResetTitle => {
                self.title.clear();
            }
            Event::ClipboardStore(clipboard, contents) => match clipboard {
                term::ClipboardType::Clipboard => self.clipboard_contents = contents,
                term::ClipboardType::Selection => self.selection_contents = contents,
            },
            Event::ClipboardLoad(clipboard, formatter) => {
                let contents = match clipboard {
                    term::ClipboardType::Clipboard => self.clipboard_contents.as_str(),
                    term::ClipboardType::Selection => self.selection_contents.as_str(),
                };
                self.write_input(formatter(contents).as_bytes());
            }
            Event::ColorRequest(index, formatter) => {
                let color = self.color_for_request(index);
                self.write_input(formatter(color).as_bytes());
            }
            Event::PtyWrite(text) => {
                self.write_input(text.as_bytes());
            }
            Event::TextAreaSizeRequest(formatter) => {
                self.write_input(formatter(self.window_size()).as_bytes());
            }
            Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }

    fn color_for_request(&self, index: usize) -> Rgb {
        self.term.lock().colors().lookup(index)
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows,
            num_cols: self.cols,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self
            .event_sender
            .send(Msg::Shutdown)
            .map_err(|error| tracing::debug!("failed to stop terminal event loop: {error}"));

        // Detach the event loop thread instead of joining it — joining can
        // block the main thread indefinitely if the child process is still
        // starting up or the PTY is stuck on I/O.
        drop(self.event_loop_handle.take());
    }
}

trait ColorLookup {
    fn lookup(&self, index: usize) -> Rgb;
}

impl ColorLookup for alacritty_terminal::term::color::Colors {
    fn lookup(&self, index: usize) -> Rgb {
        self[index].unwrap_or_else(|| default_terminal_rgb(index))
    }
}

fn default_terminal_rgb(index: usize) -> Rgb {
    if let Some(color) = TERMINAL_BASE_COLORS.get(index) {
        return *color;
    }

    match index {
        16..=231 => {
            let idx = index - 16;
            let steps = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
            Rgb {
                r: steps[idx / 36],
                g: steps[(idx % 36) / 6],
                b: steps[idx % 6],
            }
        }
        232..=255 => {
            let value = 8 + ((index - 232) * 10);
            let value = u8::try_from(value).unwrap_or(u8::MAX);
            Rgb {
                r: value,
                g: value,
                b: value,
            }
        }
        256 | 267 => Rgb { r: 224, g: 230, b: 241 },
        257 | 268 => Rgb { r: 15, g: 19, b: 28 },
        258 => Rgb { r: 196, g: 223, b: 255 },
        _ => Rgb { r: 255, g: 255, b: 255 },
    }
}

const TERMINAL_BASE_COLORS: [Rgb; 16] = [
    rgb(0x1d, 0x1f, 0x21),
    rgb(0xcc, 0x66, 0x66),
    rgb(0xb5, 0xbd, 0x68),
    rgb(0xf0, 0xc6, 0x74),
    rgb(0x81, 0xa2, 0xbe),
    rgb(0xb2, 0x94, 0xbb),
    rgb(0x8a, 0xbe, 0xb7),
    rgb(0xc5, 0xc8, 0xc6),
    rgb(0x66, 0x66, 0x66),
    rgb(0xd5, 0x4e, 0x53),
    rgb(0xb9, 0xca, 0x4a),
    rgb(0xe7, 0xc5, 0x47),
    rgb(0x7a, 0xa6, 0xda),
    rgb(0xc3, 0x97, 0xd8),
    rgb(0x70, 0xc0, 0xb1),
    rgb(0xea, 0xea, 0xea),
];

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::{TerminalDimensions, default_terminal_rgb};
    use alacritty_terminal::grid::Dimensions;

    #[test]
    fn terminal_dimensions_clamp_to_supported_minimums() {
        let dimensions = TerminalDimensions::new(0, 1);

        assert_eq!(dimensions.screen_lines(), 1);
        assert_eq!(dimensions.columns(), 2);
        assert_eq!(dimensions.total_lines(), 1);
    }

    #[test]
    fn indexed_color_cube_matches_xterm_steps() {
        let color = default_terminal_rgb(21);

        assert_eq!((color.r, color.g, color.b), (0, 0, 255));
    }
}
