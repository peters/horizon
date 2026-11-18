//! The screen and the output of a terminal for a follower that shows it elsewhere.
//!
//! A follower parses one [`ShareScreen`] and then the output that the
//! [`ShareOutlet`] of the terminal gives after it. A person types into the program
//! from there, so a screen sets the input modes that the program asked for besides
//! the text, the colours and the cursor.
//!
//! No byte is lost or doubled between a screen and the output after it. The
//! reader of the PTY holds the lease of the grid for one whole pass, and it hands
//! every byte of a pass to the parser before it gives the lease back.
//! [`Terminal::share_screen`] starts the outlet while it holds that lease and
//! keeps the grid locked until the screen is written. So the grid holds every byte
//! that the reader read before, and the outlet holds every byte that it reads
//! after. The owner calls `share_screen` and the takes that continue it on one
//! thread and sends what they give in the order of the calls: a take on a second
//! thread would answer with the output after that screen and drop the bytes
//! between its own last take and the screen, and `horizon_pty_tap::share` asserts
//! the rule in a debug build. A take says `overflowed` when the outlet lost its
//! screen or had none, and the owner answers it with a new screen.
//!
//! The parser of the reader is out of reach. A sequence that it has not finished,
//! or a synchronized update that it still holds back, reaches the follower
//! without its start, and the next screen repairs what that draws.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Weak};

use alacritty_terminal::event_loop::{EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
pub use horizon_pty_tap::{ShareOutlet, ShareTake};

use super::screen_replay::{Style, draw_rows, draw_rows_in_place, last_painted_row, push_cell, push_row};
use super::wake::OutputWakeState;
use super::{Terminal, TerminalDimensions, TerminalEventProxy, WindowSize};
use crate::agent_work::WorkOwner;

/// The bounds of the grid of a screen: the server applies them to a `screen` and
/// to a panel of a list, and a follower to its own terminal. A terminal holds two
/// columns at the least, so that is the floor of a screen.
pub(crate) const MAX_SCREEN_COLS: u16 = 1000;
pub(crate) const MAX_SCREEN_ROWS: u16 = 500;

/// The grid of a screen inside those bounds.
#[must_use]
pub fn bounded_screen_grid(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(2, MAX_SCREEN_COLS), rows.clamp(1, MAX_SCREEN_ROWS))
}

/// The most lines above the viewport that a fresh screen carries, and the most
/// bytes that they take.
const HISTORY_LINES: usize = 1_000;
const HISTORY_BYTES: usize = 256 * 1024;

/// One screen of a terminal, as a follower draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareScreen {
    pub cols: u16,
    pub rows: u16,
    /// The bytes that draw the screen and set its modes in a terminal of this
    /// grid. A fresh screen starts with a full reset, so a terminal that showed
    /// an older screen draws the same as a new one, and it starts with the lines
    /// above the viewport. A screen that is not fresh starts with none of them:
    /// the follower keeps its history, and it takes the grid in place.
    pub bytes: Vec<u8>,
}

/// Each mode with the sequence that sets it and the one that takes it away.
const MODES: [(TermMode, &str, &str); 15] = [
    (TermMode::SHOW_CURSOR, "\x1b[?25h", "\x1b[?25l"),
    (TermMode::APP_CURSOR, "\x1b[?1h", "\x1b[?1l"),
    (TermMode::APP_KEYPAD, "\x1b=", "\x1b>"),
    (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h", "\x1b[?1000l"),
    (TermMode::MOUSE_DRAG, "\x1b[?1002h", "\x1b[?1002l"),
    (TermMode::MOUSE_MOTION, "\x1b[?1003h", "\x1b[?1003l"),
    (TermMode::UTF8_MOUSE, "\x1b[?1005h", "\x1b[?1005l"),
    (TermMode::SGR_MOUSE, "\x1b[?1006h", "\x1b[?1006l"),
    (TermMode::FOCUS_IN_OUT, "\x1b[?1004h", "\x1b[?1004l"),
    (TermMode::BRACKETED_PASTE, "\x1b[?2004h", "\x1b[?2004l"),
    (TermMode::ALTERNATE_SCROLL, "\x1b[?1007h", "\x1b[?1007l"),
    (TermMode::URGENCY_HINTS, "\x1b[?1042h", "\x1b[?1042l"),
    (TermMode::LINE_WRAP, "\x1b[?7h", "\x1b[?7l"),
    (TermMode::INSERT, "\x1b[4h", "\x1b[4l"),
    (TermMode::LINE_FEED_NEW_LINE, "\x1b[20h", "\x1b[20l"),
];

/// The flags of the keyboard protocol with the numbers that a program pushes.
const KEYBOARD: [(TermMode, u8); 5] = [
    (TermMode::DISAMBIGUATE_ESC_CODES, 1),
    (TermMode::REPORT_EVENT_TYPES, 2),
    (TermMode::REPORT_ALTERNATE_KEYS, 4),
    (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
    (TermMode::REPORT_ASSOCIATED_TEXT, 16),
];

/// The grid that a follower of one terminal holds, and the cell size that the
/// program of the terminal gets with it. The thread that shares the terminal sets
/// both, and the window of the owner reads them.
#[derive(Default)]
pub(super) struct HeldGrid {
    /// Columns in the high half and rows in the low half, or zero while the
    /// program has the grid of the body of the panel.
    grid: AtomicU32,
    /// The cell width in the high half and the cell height in the low half.
    cell: AtomicU32,
}

impl HeldGrid {
    /// The grid that a follower holds, or `None` for the grid of the body.
    pub(super) fn get(&self) -> Option<(u16, u16)> {
        let value = self.grid.load(Ordering::Acquire);
        let columns = u16::try_from(value >> 16).unwrap_or_default();
        let rows = u16::try_from(value & 0xffff).unwrap_or_default();
        (value != 0).then_some((columns, rows))
    }

    /// Takes or ends the hold, and answers whether the grid changed.
    fn set(&self, grid: Option<(u16, u16)>) -> bool {
        let value = grid.map_or(0, |(columns, rows)| (u32::from(columns) << 16) | u32::from(rows));
        self.grid.swap(value, Ordering::AcqRel) != value
    }

    /// Takes the cell size of a resize, which the program gets with a grid.
    pub(super) fn set_cell(&self, width: u16, height: u16) {
        self.cell
            .store((u32::from(width) << 16) | u32::from(height), Ordering::Release);
    }

    fn get_cell(&self) -> (u16, u16) {
        let cell = self.cell.load(Ordering::Acquire);
        let (width, height) = (cell >> 16, cell & 0xffff);
        (
            u16::try_from(width).unwrap_or_default(),
            u16::try_from(height).unwrap_or_default(),
        )
    }
}

/// What another thread needs of one terminal to share it: the outlet of its
/// output, the way to its screen and the way to its program. A terminal gives
/// it once, and every call is safe on a thread that is not the thread of the
/// window. The source of a terminal that is gone gives no screen.
#[derive(Clone)]
pub struct ShareSource {
    outlet: ShareOutlet,
    term: Weak<FairMutex<Term<TerminalEventProxy>>>,
    input: EventLoopSender,
    work: Option<Arc<WorkOwner>>,
    touched: Arc<AtomicBool>,
    wake: Arc<OutputWakeState>,
    held: Arc<HeldGrid>,
}

impl ShareSource {
    /// The outlet of the output of the terminal.
    #[must_use]
    pub fn outlet(&self) -> &ShareOutlet {
        &self.outlet
    }

    /// The screen of the terminal now, as [`Terminal::share_screen`] gives it.
    #[must_use]
    pub fn screen(&self, fresh: bool) -> Option<ShareScreen> {
        Some(screen_of(&*self.term.upgrade()?, &self.outlet, fresh))
    }

    /// Writes the keys of a person at another computer into the program, as
    /// [`Terminal::write_input`] writes the keys of this computer. The first
    /// output after them asks the window for the frame that shows their echo.
    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.touched.store(true, Ordering::Release);
        if let Some(work) = &self.work {
            work.note_input(bytes);
        }
        self.wake.arm();
        let _ = self.input.send(Msg::Input(Cow::Owned(bytes.to_vec())));
    }

    /// Whether `other` is a source of the same terminal.
    #[must_use]
    pub fn same_terminal(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.term, &other.term)
    }

    /// Gives the program of the terminal the grid that a follower holds, and
    /// takes the grid of the terminal for it: the panel of the owner then pushes
    /// no size of its own body, and draws the grid from the top left. The thread
    /// that shares the terminal calls this, and a later screen carries the grid.
    /// Answers whether the grid changed.
    pub fn hold_grid(&self, cols: u16, rows: u16) -> bool {
        let Some(term) = self.term.upgrade() else {
            return false;
        };
        let (cols, rows) = bounded_screen_grid(cols, rows);
        if !self.held.set(Some((cols, rows))) {
            return false;
        }
        let (cell_width, cell_height) = self.held.get_cell();
        term.lock().resize(TerminalDimensions::new(rows, cols));
        // The followers draw the grid of the screen that they have.
        self.outlet.stop();
        let _ = self.input.send(Msg::Resize(WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width,
            cell_height,
        }));
        true
    }
}

impl Terminal {
    /// The outlet of the output of this terminal for its followers.
    #[must_use]
    pub fn share_outlet(&self) -> ShareOutlet {
        self.share.clone()
    }

    /// The screen of this terminal now. The outlet starts at the same instant,
    /// so what it gives after this call continues this screen. A `fresh` screen
    /// holds the lines above the viewport; one that is not fresh draws the grid
    /// in place and leaves the history of the follower alone.
    #[must_use]
    pub fn share_screen(&self, fresh: bool) -> ShareScreen {
        screen_of(&self.term, &self.share, fresh)
    }

    /// The source that shares this terminal from another thread, or `None` for
    /// a follower, which shows a terminal of another computer.
    #[must_use]
    pub fn share_source(&self) -> Option<ShareSource> {
        self.follow.is_none().then(|| ShareSource {
            outlet: self.share.clone(),
            term: Arc::downgrade(&self.term),
            input: self.event_sender.clone(),
            work: self.work_owner.clone(),
            touched: Arc::clone(&self.work_continuation.touched),
            wake: Arc::clone(&self.output_wake),
            held: Arc::clone(&self.held_grid),
        })
    }

    /// Rows and columns of the grid that this terminal shows now: the grid of its
    /// stream while it follows one, or the grid that a follower of it holds, else
    /// the grid of its own body.
    #[must_use]
    pub fn shown_grid(&self) -> (u16, u16) {
        self.held_grid
            .get()
            .map_or((self.rows, self.cols), |(columns, rows)| (rows, columns))
    }

    /// Whether the grid of this terminal is not the grid of its own body: it
    /// follows a stream, or a follower of it holds the size of its program.
    #[must_use]
    pub fn shows_foreign_grid(&self) -> bool {
        self.follow.is_some() || self.held_grid.get().is_some()
    }

    /// The person of this computer types in the panel, so it takes the size back:
    /// the grid of the body goes to the terminal and to the program again.
    pub(super) fn release_grid(&self) {
        if !self.held_grid.set(None) {
            return;
        }
        self.term.lock().resize(TerminalDimensions::new(self.rows, self.cols));
        self.share.stop();
        let _ = self.event_sender.send(Msg::Resize(self.window_size()));
    }
}

/// The screen of `term` now, with `outlet` started at the same instant.
fn screen_of(term: &FairMutex<Term<TerminalEventProxy>>, outlet: &ShareOutlet, fresh: bool) -> ShareScreen {
    // The reader is between two passes while this holds the lease, and the
    // grid lock is taken before the lease goes back. The module says why
    // that leaves no byte out and none twice.
    let lease = term.lease();
    outlet.start();
    let term = term.lock_unfair();
    drop(lease);
    ShareScreen {
        cols: u16::try_from(term.columns()).unwrap_or(u16::MAX),
        rows: u16::try_from(term.screen_lines()).unwrap_or(u16::MAX),
        bytes: share_screen_bytes(&term, fresh),
    }
}

/// The bytes of a [`ShareScreen`] of `term`.
pub(super) fn share_screen_bytes(term: &Term<TerminalEventProxy>, fresh: bool) -> Vec<u8> {
    let (grid, mode) = (term.grid(), *term.mode());
    let mut out = String::new();
    if fresh {
        out.push_str("\x1bc");
        if mode.contains(TermMode::ALT_SCREEN) {
            out.push_str("\x1b[?1049h");
        } else {
            // The program is on the primary screen, so the lines that it scrolled
            // off are there to read, and a new follower gets them above its
            // viewport. The alternate screen of a program holds no history.
            draw_history(grid, &mut out);
        }
        // The rows are drawn with the modes of a new terminal, and every mode
        // that changes how output is drawn is set after them.
        draw_rows(grid, &mut out);
        // `draw_rows` ends at the last row that paints anything. The rows below
        // that one take their place too, so that the viewport of the follower
        // ends at the bottom row, directly under the lines above it.
        for _ in last_painted_row(grid, grid.screen_lines()) + 1..grid.screen_lines() {
            out.push_str("\r\n");
        }
    } else {
        // The follower keeps its history, so the screen may not start with a full
        // reset and may not scroll: each row goes to its own place. The alternate
        // screen comes first, because the rows belong on the screen that it names.
        out.push_str(if mode.contains(TermMode::ALT_SCREEN) {
            "\x1b[?1049h"
        } else {
            "\x1b[?1049l"
        });
        draw_rows_in_place(grid, &mut out);
    }
    if mode.contains(TermMode::ORIGIN) {
        out.push_str("\x1b[?6h");
    } else if !fresh {
        // The origin mode moves the cursor, so it is no mode of the table below,
        // and a follower that keeps its modes must be taken out of it.
        out.push_str("\x1b[?6l");
    }
    let cursor = &grid.cursor;
    let row = &grid[cursor.point.line];
    let mut column = cursor.point.column;
    if cursor.input_needs_wrap && row[column].flags.contains(Flags::WIDE_CHAR_SPACER) {
        column = Column(column.0.saturating_sub(1));
    }
    let _ = write!(out, "\x1b[{};{}H", cursor.point.line.0 + 1, column.0 + 1);
    let mut pen = Style::default();
    if cursor.input_needs_wrap {
        // The cursor waits behind the last column for the next character. The
        // cell under it, drawn once more, leaves the cursor of the follower there.
        pen.paint(&mut out, &row[column]);
        push_cell(&mut out, &row[column]);
    }
    pen.paint(&mut out, &cursor.template);
    for (flag, set, unset) in MODES {
        let on = mode.contains(flag);
        if on == TermMode::default().contains(flag) && fresh {
            // The reset at the start of a fresh screen already set this mode.
            continue;
        }
        out.push_str(if on { set } else { unset });
    }
    let keyboard: u8 = KEYBOARD
        .iter()
        .filter(|key| mode.contains(key.0))
        .map(|key| key.1)
        .sum();
    if keyboard != 0 || !fresh {
        // A fresh screen starts with no flag. One that is not fresh replaces the
        // flags that the follower has, without pushing a level of its stack.
        let _ = write!(out, "\x1b[{}{keyboard}u", if fresh { '>' } else { '=' });
    }
    let style = term.cursor_style();
    let shape = match style.shape {
        CursorShape::Block => 2,
        CursorShape::Underline => 4,
        CursorShape::Beam => 6,
        CursorShape::HollowBlock | CursorShape::Hidden => 0,
    };
    if shape != 0 && style != CursorStyle::default() {
        let _ = write!(out, "\x1b[{} q", shape - u8::from(style.blinking));
    } else if !fresh {
        let _ = write!(out, "\x1b[0 q");
    }
    out.into_bytes()
}

/// The lines above the viewport of `grid`, oldest first, as rows that a fresh
/// screen draws before the viewport: they scroll into the scrollback of the
/// follower, and its viewport ends under them.
fn draw_history(grid: &Grid<Cell>, out: &mut String) {
    let mut painted = Style::default();
    let mut lines: Vec<String> = Vec::new();
    let mut bytes = 0;
    for line in
        (1..=grid.history_size().min(HISTORY_LINES)).map(|above| Line(-i32::try_from(above).unwrap_or(i32::MAX)))
    {
        let mut drawn = String::new();
        push_row(&mut drawn, &grid[line], &mut painted, grid.columns());
        if bytes + drawn.len() > HISTORY_BYTES {
            break;
        }
        bytes += drawn.len() + 2;
        lines.push(drawn);
    }
    // The lines closest to the viewport are the ones that a person scrolls to.
    for line in lines.iter().rev() {
        out.push_str(line);
        out.push_str("\r\n");
    }
}

#[cfg(test)]
mod tests;
