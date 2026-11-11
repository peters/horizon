//! The visible screen of a terminal as output that draws it again.
//!
//! A machine terminal that reconnects starts its command again in a new
//! terminal, and a new terminal has an empty grid. The person read the work of
//! that screen, so the reconnect hands the new terminal what the old one showed:
//! the rows of the viewport, their colours and their styles. The connection
//! draws over them when it arrives.

use std::fmt::Write as _;

use alacritty_terminal::grid::{Dimensions, Grid, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use super::{Terminal, TerminalEventProxy};

impl Terminal {
    /// The visible screen of this terminal as bytes that a new terminal can
    /// replay, so a restart keeps what the person read.
    ///
    /// The bytes are the text of the viewport with the colours of each run and
    /// no screen mode: a replay that entered the alternate screen would push
    /// the text onto a screen that the connection never draws on.
    #[must_use]
    pub fn screen_replay_bytes(&self) -> Vec<u8> {
        let term = self.term.lock();
        screen_replay_bytes(&term)
    }
}

/// The bytes that draw the viewport of `term` again, as
/// [`Terminal::screen_replay_bytes`] describes them.
pub(super) fn screen_replay_bytes(term: &Term<TerminalEventProxy>) -> Vec<u8> {
    let grid = term.grid();
    let rows = grid.screen_lines();
    let columns = grid.columns();
    let mut out = String::with_capacity(rows * (columns + 2));
    let mut painted = Style::default();
    let last_row = last_painted_row(grid, rows);
    for row in 0..=last_row {
        let line = &grid[Line(i32::try_from(row).unwrap_or(i32::MAX))];
        if let Some(last_column) = last_painted_column(line, columns) {
            for column in 0..=last_column {
                let cell = &line[Column(column)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    continue;
                }
                painted.paint(&mut out, cell);
                out.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    out.extend(extra.iter());
                }
            }
        }
        painted = painted.reset(&mut out);
        if row < last_row {
            out.push_str("\r\n");
        }
    }
    out.into_bytes()
}

/// The last row of the viewport that paints anything, so the replay ends at the
/// content instead of scrolling it away with empty rows.
fn last_painted_row(grid: &Grid<Cell>, rows: usize) -> usize {
    (0..rows)
        .rev()
        .find(|row| last_painted_column(&grid[Line(i32::try_from(*row).unwrap_or(i32::MAX))], grid.columns()).is_some())
        .unwrap_or(0)
}

/// The last column of `line` that paints anything. A blank cell with the default
/// colours paints nothing, so the replay stops before it.
fn last_painted_column(line: &Row<Cell>, columns: usize) -> Option<usize> {
    line[..Column(columns)].iter().rposition(|cell| {
        cell.c != ' '
            || !matches!(cell.bg, Color::Named(NamedColor::Background))
            || cell
                .flags
                .intersects(Flags::UNDERLINE | Flags::INVERSE | Flags::STRIKEOUT)
    })
}

/// The colours and the attributes of the run that a replay has drawn so far, so
/// a run keeps them and only a change writes an escape sequence.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Option<Wanted>,
    bg: Option<Wanted>,
    attributes: u16,
}

/// One colour as an SGR parameter list, or `None` for the terminal default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wanted {
    Ansi(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

const BOLD: u16 = 1;
const DIM: u16 = 2;
const ITALIC: u16 = 4;
const UNDERLINE: u16 = 8;
const REVERSE: u16 = 16;
const STRIKEOUT: u16 = 32;

impl Style {
    /// The style of `cell`, as the screen shows it.
    fn of(cell: &Cell) -> Self {
        Self {
            fg: wanted(cell.fg, NamedColor::Foreground),
            bg: wanted(cell.bg, NamedColor::Background),
            attributes: attributes(cell.flags),
        }
    }

    /// Writes the escape sequence that makes this style the current one, when it
    /// is not already current.
    fn paint(&mut self, out: &mut String, cell: &Cell) {
        let style = Self::of(cell);
        if style == *self {
            return;
        }
        out.push_str("\x1b[0");
        for (mask, parameter) in [
            (BOLD, "1"),
            (DIM, "2"),
            (ITALIC, "3"),
            (UNDERLINE, "4"),
            (REVERSE, "7"),
            (STRIKEOUT, "9"),
        ] {
            if style.attributes & mask != 0 {
                out.push(';');
                out.push_str(parameter);
            }
        }
        write_color(out, ColorKind::Foreground, style.fg);
        write_color(out, ColorKind::Background, style.bg);
        out.push('m');
        *self = style;
    }

    /// The default style, written when it is not current yet.
    fn reset(&self, out: &mut String) -> Self {
        if *self != Self::default() {
            out.push_str("\x1b[0m");
        }
        Self::default()
    }
}

fn attributes(flags: Flags) -> u16 {
    let mut attributes = 0;
    for (flag, mask) in [
        (Flags::BOLD, BOLD),
        (Flags::DIM, DIM),
        (Flags::ITALIC, ITALIC),
        (Flags::UNDERLINE, UNDERLINE),
        (Flags::INVERSE, REVERSE),
        (Flags::STRIKEOUT, STRIKEOUT),
    ] {
        if flags.contains(flag) {
            attributes |= mask;
        }
    }
    attributes
}

fn wanted(color: Color, default: NamedColor) -> Option<Wanted> {
    match color {
        Color::Spec(rgb) => Some(Wanted::Rgb(rgb.r, rgb.g, rgb.b)),
        Color::Named(named) if named == default => None,
        Color::Named(named) => ansi_index(named).map(Wanted::Ansi),
        Color::Indexed(index) => Some(Wanted::Indexed(index)),
    }
}

/// The ANSI parameter of one of the sixteen colours of the palette. A colour
/// that the palette names outside those sixteen is no colour that a replay can
/// ask for, so it becomes the default.
const fn ansi_index(named: NamedColor) -> Option<u8> {
    Some(match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimBlack
        | NamedColor::DimRed
        | NamedColor::DimGreen
        | NamedColor::DimYellow
        | NamedColor::DimBlue
        | NamedColor::DimMagenta
        | NamedColor::DimCyan
        | NamedColor::DimWhite
        | NamedColor::DimForeground => return None,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorKind {
    Foreground,
    Background,
}

impl ColorKind {
    const fn extended(self) -> &'static str {
        match self {
            Self::Foreground => "38",
            Self::Background => "48",
        }
    }

    const fn base(self) -> u8 {
        match self {
            Self::Foreground => 30,
            Self::Background => 40,
        }
    }

    const fn bright(self) -> u8 {
        match self {
            Self::Foreground => 90,
            Self::Background => 100,
        }
    }
}

fn write_color(out: &mut String, kind: ColorKind, color: Option<Wanted>) {
    let Some(color) = color else {
        return;
    };
    match color {
        Wanted::Ansi(index) if index < 8 => {
            let _ = write!(out, ";{}", kind.base() + index);
        }
        Wanted::Ansi(index) => {
            let _ = write!(out, ";{}", kind.bright() + index - 8);
        }
        Wanted::Indexed(index) => {
            let _ = write!(out, ";{};5;{index}", kind.extended());
        }
        Wanted::Rgb(r, g, b) => {
            let _ = write!(out, ";{};2;{r};{g};{b}", kind.extended());
        }
    }
}

#[cfg(test)]
mod tests;
