use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, State};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, RenderableContent, Term, TermDamage, TermMode, viewport_to_point};
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
use alacritty_terminal::vte::ansi::{self, Rgb};

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
    pub replay_bytes: Vec<u8>,
    pub env: HashMap<String, String>,
}

/// A structured notification parsed from an OSC title sequence.
#[derive(Clone, Debug)]
pub struct AgentNotification {
    pub severity: String,
    pub message: String,
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
    child_pid: Option<u32>,
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
    child_exited: bool,
    bell_pending: bool,
    pending_notification: Option<AgentNotification>,
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
        let replay_bytes = options.replay_bytes;
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
            env: options.env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let term = Arc::new(FairMutex::new(Term::new(terminal_config, &dimensions, term_proxy)));
        if !replay_bytes.is_empty() {
            replay_terminal_bytes(&term, &replay_bytes);
        }
        let pty =
            tty::new(&pty_options, window_size, options.window_id).map_err(|error| Error::Pty(error.to_string()))?;
        #[cfg(not(windows))]
        let child_pid = Some(pty.child().id());
        #[cfg(windows)]
        let child_pid = None;
        let event_loop = EventLoop::new(term.clone(), event_loop_proxy, pty, true, false)
            .map_err(|error| Error::Pty(format!("failed to initialize terminal event loop: {error}")))?;
        let event_sender = event_loop.channel();
        let event_loop_handle = Some(event_loop.spawn());

        let mut terminal = Self {
            term,
            event_sender,
            event_rx,
            event_loop_handle,
            child_pid,
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
            child_exited: false,
            bell_pending: false,
            pending_notification: None,
        };
        terminal.process_events();
        Ok(terminal)
    }

    /// Drain pending PTY events. Returns `true` if any events were processed.
    pub fn process_events(&mut self) -> bool {
        let mut had_events = false;
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
            had_events = true;
        }
        self.flush_pending_pty_resize();
        had_events
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

    pub fn request_shutdown(&mut self) {
        if self.event_loop_handle.is_none() {
            return;
        }

        let _ = self
            .event_sender
            .send(Msg::Shutdown)
            .map_err(|error| tracing::debug!("failed to stop terminal event loop: {error}"));
    }

    #[must_use]
    pub fn wait_for_shutdown(&mut self, timeout: Duration) -> bool {
        enum JoinStatus {
            Complete,
            Panicked,
        }

        let Some(event_loop_handle) = self.event_loop_handle.take() else {
            return true;
        };

        let (shutdown_tx, shutdown_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            // Drop the joined event loop on this helper thread so PTY teardown
            // cannot block the UI thread in `Pty::drop`.
            let status = match event_loop_handle.join() {
                Ok(_) => JoinStatus::Complete,
                Err(_) => JoinStatus::Panicked,
            };
            let _ = shutdown_tx.send(status);
        });

        match shutdown_rx.recv_timeout(timeout) {
            Ok(JoinStatus::Complete) => true,
            Ok(JoinStatus::Panicked) => {
                tracing::warn!("terminal event loop panicked during shutdown");
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => false,
        }
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        self.request_shutdown();
        self.wait_for_shutdown(timeout)
    }

    /// Spawns a background thread to join the event-loop handle, incrementing
    /// `completed` when done.  Returns `true` if a join thread was spawned.
    pub(crate) fn begin_async_join(&mut self, completed: &Arc<AtomicUsize>) -> bool {
        let Some(handle) = self.event_loop_handle.take() else {
            return false;
        };
        let done = Arc::clone(completed);
        std::thread::spawn(move || {
            // Join and drop on this helper thread so PTY teardown
            // cannot block the UI thread.
            let _ = handle.join();
            done.fetch_add(1, Ordering::Relaxed);
        });
        true
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
    pub fn current_cwd(&self) -> Option<PathBuf> {
        current_cwd_for_pid(self.child_pid?)
    }

    #[must_use]
    pub fn child_exited(&self) -> bool {
        self.child_exited
    }

    /// Returns `true` if a bell has fired since the last call, then clears.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_pending)
    }

    pub fn take_notification(&mut self) -> Option<AgentNotification> {
        self.pending_notification.take()
    }

    fn parse_horizon_notification(title: &str) -> Option<AgentNotification> {
        let payload = title.strip_prefix("HORIZON_NOTIFY:")?;
        let (severity, message) = payload.split_once(':')?;
        if message.is_empty() {
            return None;
        }
        Some(AgentNotification {
            severity: severity.to_string(),
            message: message.to_string(),
        })
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

    /// Start a new text selection at the given viewport-relative row and column.
    pub fn start_selection(&self, sel_type: SelectionType, row: usize, col: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset();
        let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
        let side = Side::Left;
        term.selection = Some(Selection::new(sel_type, point, side));
    }

    /// Update the active selection to the given viewport-relative row and column.
    pub fn update_selection(&self, row: usize, col: usize, side: Side) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset();
        let point = viewport_to_point(display_offset, Point::new(row, Column(col)));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
            selection.include_all();
        }
    }

    /// Clear any active selection.
    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// Return whether a selection is currently active.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.term.lock().selection.is_some()
    }

    /// Extract the currently selected text, if any.
    #[must_use]
    pub fn selection_to_string(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Title(title) => {
                if let Some(notification) = Self::parse_horizon_notification(&title) {
                    self.pending_notification = Some(notification);
                } else {
                    self.title = title;
                }
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
            Event::Exit | Event::ChildExit(_) => {
                self.child_exited = true;
            }
            Event::Bell => {
                self.bell_pending = true;
            }
            Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Wakeup => {}
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
        self.request_shutdown();

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

fn replay_terminal_bytes(term: &Arc<FairMutex<Term<TerminalEventProxy>>>, bytes: &[u8]) {
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    let mut terminal = term.lock();
    parser.advance(&mut *terminal, bytes);

    // Persisted transcripts can end while a fullscreen program has mouse
    // reporting, the alternate screen, or application keypad modes enabled.
    // Those modes must be re-established by the live PTY session, not carried
    // over from replay, otherwise restored shell panels can inherit stale
    // click/key handling state.
    let reset_bytes = replay_mode_reset_bytes(*terminal.mode());
    if !reset_bytes.is_empty() {
        parser.advance(&mut *terminal, &reset_bytes);
    }
}

fn replay_mode_reset_bytes(mode: TermMode) -> Vec<u8> {
    let mut bytes = Vec::new();

    if mode.contains(TermMode::APP_CURSOR) {
        bytes.extend_from_slice(b"\x1b[?1l");
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        bytes.extend_from_slice(b"\x1b>");
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        bytes.extend_from_slice(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
    }
    if mode.contains(TermMode::FOCUS_IN_OUT) {
        bytes.extend_from_slice(b"\x1b[?1004l");
    }
    if mode.contains(TermMode::UTF8_MOUSE) {
        bytes.extend_from_slice(b"\x1b[?1005l");
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        bytes.extend_from_slice(b"\x1b[?1006l");
    }
    if mode.contains(TermMode::ALT_SCREEN) {
        bytes.extend_from_slice(b"\x1b[?1049l");
    }
    if !mode.contains(TermMode::SHOW_CURSOR) {
        bytes.extend_from_slice(b"\x1b[?25h");
    }

    bytes
}

fn current_cwd_for_pid(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
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
    use std::time::Duration;

    use std::collections::HashMap;

    use super::{
        Terminal, TerminalDimensions, TerminalEventProxy, TerminalSpawnOptions, current_cwd_for_pid,
        default_terminal_rgb, replay_terminal_bytes,
    };
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::sync::FairMutex;
    use alacritty_terminal::term::{self, Term, TermMode};
    use std::sync::Arc;
    use std::sync::mpsc;

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

    #[test]
    fn shutdown_with_timeout_waits_for_pty_exit() {
        let mut terminal = Terminal::spawn(TerminalSpawnOptions {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
            args: Vec::new(),
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 256,
            window_id: 41,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
        })
        .expect("terminal should spawn");

        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn current_cwd_for_pid_reads_procfs_cwd() {
        let cwd = current_cwd_for_pid(std::process::id()).expect("cwd");
        assert_eq!(cwd, std::env::current_dir().expect("current dir"));
    }

    #[test]
    fn replay_clears_stale_fullscreen_modes_from_transcripts() {
        let term = test_term();
        replay_terminal_bytes(
            &term,
            b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b=\x1b[?1h\x1b[?25l",
        );

        let mode = *term.lock().mode();
        assert_eq!(mode, TermMode::default());
    }

    fn test_term() -> Arc<FairMutex<Term<TerminalEventProxy>>> {
        let (event_tx, _event_rx) = mpsc::channel();
        let dimensions = TerminalDimensions::new(24, 80);
        let config = term::Config {
            scrolling_history: 256,
            kitty_keyboard: true,
            ..term::Config::default()
        };

        Arc::new(FairMutex::new(Term::new(
            config,
            &dimensions,
            TerminalEventProxy { event_tx },
        )))
    }
}
