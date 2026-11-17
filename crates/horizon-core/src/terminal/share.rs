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

use std::fmt::Write as _;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
pub use horizon_pty_tap::{ShareOutlet, ShareTake};

use super::screen_replay::{Style, draw_rows, push_cell};
use super::{Terminal, TerminalEventProxy};

/// One screen of a terminal, as a follower draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareScreen {
    pub cols: u16,
    pub rows: u16,
    /// The bytes that draw the screen and set its modes in a terminal of this
    /// grid. They start with a full reset, so a terminal that showed an older
    /// screen draws the same as a new one.
    pub bytes: Vec<u8>,
}

/// Each mode with the sequence that takes it away from its state in a new
/// terminal. The origin mode is not here, because it moves the cursor.
const MODES: [(TermMode, &str); 15] = [
    (TermMode::SHOW_CURSOR, "\x1b[?25l"),
    (TermMode::APP_CURSOR, "\x1b[?1h"),
    (TermMode::APP_KEYPAD, "\x1b="),
    (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
    (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
    (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
    (TermMode::UTF8_MOUSE, "\x1b[?1005h"),
    (TermMode::SGR_MOUSE, "\x1b[?1006h"),
    (TermMode::FOCUS_IN_OUT, "\x1b[?1004h"),
    (TermMode::BRACKETED_PASTE, "\x1b[?2004h"),
    (TermMode::ALTERNATE_SCROLL, "\x1b[?1007l"),
    (TermMode::URGENCY_HINTS, "\x1b[?1042l"),
    (TermMode::LINE_WRAP, "\x1b[?7l"),
    (TermMode::INSERT, "\x1b[4h"),
    (TermMode::LINE_FEED_NEW_LINE, "\x1b[20h"),
];

/// The flags of the keyboard protocol with the numbers that a program pushes.
const KEYBOARD: [(TermMode, u8); 5] = [
    (TermMode::DISAMBIGUATE_ESC_CODES, 1),
    (TermMode::REPORT_EVENT_TYPES, 2),
    (TermMode::REPORT_ALTERNATE_KEYS, 4),
    (TermMode::REPORT_ALL_KEYS_AS_ESC, 8),
    (TermMode::REPORT_ASSOCIATED_TEXT, 16),
];

impl Terminal {
    /// The outlet of the output of this terminal for its followers.
    #[must_use]
    pub fn share_outlet(&self) -> ShareOutlet {
        self.share.clone()
    }

    /// The screen of this terminal now. The outlet starts at the same instant,
    /// so what it gives after this call continues this screen.
    #[must_use]
    pub fn share_screen(&self) -> ShareScreen {
        // The reader is between two passes while this holds the lease, and the
        // grid lock is taken before the lease goes back. The module says why
        // that leaves no byte out and none twice.
        let lease = self.term.lease();
        self.share.start();
        let term = self.term.lock_unfair();
        drop(lease);
        ShareScreen {
            cols: u16::try_from(term.columns()).unwrap_or(u16::MAX),
            rows: u16::try_from(term.screen_lines()).unwrap_or(u16::MAX),
            bytes: share_screen_bytes(&term),
        }
    }
}

/// The bytes of a [`ShareScreen`] of `term`.
pub(super) fn share_screen_bytes(term: &Term<TerminalEventProxy>) -> Vec<u8> {
    let (grid, mode) = (term.grid(), *term.mode());
    let mut out = String::from("\x1bc");
    if mode.contains(TermMode::ALT_SCREEN) {
        out.push_str("\x1b[?1049h");
    }
    // The rows are drawn with the modes of a new terminal, and every mode that
    // changes how output is drawn is set after them.
    draw_rows(grid, &mut out, true);
    if mode.contains(TermMode::ORIGIN) {
        out.push_str("\x1b[?6h");
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
        push_cell(&mut out, &row[column], true);
    }
    pen.paint(&mut out, &cursor.template);
    for (flag, sequence) in MODES {
        if (mode ^ TermMode::default()).contains(flag) {
            out.push_str(sequence);
        }
    }
    let keyboard: u8 = KEYBOARD
        .iter()
        .filter(|key| mode.contains(key.0))
        .map(|key| key.1)
        .sum();
    if keyboard != 0 {
        let _ = write!(out, "\x1b[>{keyboard}u");
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
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests;
