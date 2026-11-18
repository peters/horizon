//! The screen of a follower against the owner.
//!
//! These tests draw into a terminal without a program and compare what a follower
//! shows after it parsed the screen. The module `program` runs a program on a real
//! pseudo terminal and follows it through the outlet.

use std::fmt::Write as _;
use std::sync::{Arc, mpsc};

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};

use super::share_screen_bytes;
use crate::terminal::support::replay_live_bytes;
use crate::terminal::{TerminalDimensions, TerminalEventProxy};

type Screen = Arc<FairMutex<Term<TerminalEventProxy>>>;

/// A terminal without a program, which tracks the keyboard protocol as the
/// terminal of a shell does.
fn screen(rows: u16, cols: u16) -> Screen {
    let (event_tx, _event_rx) = mpsc::channel::<Event>();
    Arc::new(FairMutex::new(Term::new(
        term::Config {
            scrolling_history: 1000,
            kitty_keyboard: true,
            ..term::Config::default()
        },
        &TerminalDimensions::new(rows, cols),
        TerminalEventProxy::for_tests(event_tx),
    )))
}

/// The owner that drew `output`, and a new follower that parsed its screen.
fn followed(rows: u16, cols: u16, output: &[u8]) -> (Screen, Screen) {
    let owner = screen(rows, cols);
    replay_live_bytes(&owner, output);
    let follower = screen(rows, cols);
    follow(&owner, &follower);
    (owner, follower)
}

/// Hands the screen of `owner` to `follower`, which may have shown another one.
fn follow(owner: &Screen, follower: &Screen) {
    let bytes = share_screen_bytes(&owner.lock(), true);
    replay_live_bytes(follower, &bytes);
}

/// The style of a cell that a person sees.
fn style(flags: Flags) -> Flags {
    flags
        & (Flags::BOLD
            | Flags::DIM
            | Flags::ITALIC
            | Flags::UNDERLINE
            | Flags::INVERSE
            | Flags::STRIKEOUT
            | Flags::WIDE_CHAR
            | Flags::WIDE_CHAR_SPACER)
}

/// What a person sees of one cell. The colour of the text and its weight show
/// nothing in a blank cell, so a blank is the same blank on both sides.
fn seen(cell: &Cell) -> (char, Vec<char>, Color, Color, Flags) {
    let flags = style(cell.flags);
    if cell.c == ' ' && !flags.intersects(Flags::UNDERLINE | Flags::INVERSE | Flags::STRIKEOUT) {
        return (
            ' ',
            Vec::new(),
            Color::Named(NamedColor::Foreground),
            cell.bg,
            Flags::empty(),
        );
    }
    let extra = cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default();
    (cell.c, extra, cell.fg, cell.bg, flags)
}

/// The text of every row of `term`, without the blanks at the end of a row.
fn rows_text(term: &Term<TerminalEventProxy>) -> Vec<String> {
    (0..term.screen_lines())
        .map(|row| {
            let line = &term.grid()[Line(i32::try_from(row).expect("a row of the screen"))];
            let text: String = line[..Column(term.columns())].iter().map(|cell| cell.c).collect();
            text.trim_end().to_owned()
        })
        .collect()
}

/// Asserts that `follower` shows the screen of `owner`: every cell, the cursor
/// with its wait behind the last column, the pen, the modes and the cursor shape.
fn assert_same(owner: &Screen, follower: &Screen) {
    let (owner, follower) = (owner.lock(), follower.lock());
    assert_eq!(
        (owner.screen_lines(), owner.columns()),
        (follower.screen_lines(), follower.columns())
    );
    for row in 0..owner.screen_lines() {
        let line = Line(i32::try_from(row).expect("a row of the screen"));
        let cells = |term: &Term<TerminalEventProxy>| -> Vec<_> {
            term.grid()[line][..Column(term.columns())].iter().map(seen).collect()
        };
        assert_eq!(cells(&owner), cells(&follower), "row {row} of the screen");
    }
    let (ours, theirs) = (&owner.grid().cursor, &follower.grid().cursor);
    assert_eq!(ours.point, theirs.point, "the cursor");
    assert_eq!(
        ours.input_needs_wrap, theirs.input_needs_wrap,
        "the wait behind the last column"
    );
    let pen = |cell: &Cell| (cell.fg, cell.bg, style(cell.flags));
    assert_eq!(pen(&ours.template), pen(&theirs.template), "the pen");
    assert_eq!(*owner.mode(), *follower.mode(), "the modes");
    assert_eq!(owner.cursor_style(), follower.cursor_style(), "the cursor shape");
}

/// Draws `output` on both sides, as the output after a screen reaches them.
fn draw_on_both(owner: &Screen, follower: &Screen, output: &[u8]) {
    replay_live_bytes(owner, output);
    replay_live_bytes(follower, output);
}

#[test]
fn a_follower_shows_the_text_the_colours_and_the_cursor_of_the_owner() {
    let output = "plain \x1b[1;31mbold red\x1b[0m \x1b[3;4;38;5;208;48;2;10;20;30mfancy\x1b[0m\r\n\
                  wide \u{4f60}\u{597d} and e\u{301}\r\n\
                  \x1b[7minverse\x1b[0m \x1b[2;9mdim struck\x1b[0m\
                  \x1b[10;20H\x1b[44m     \x1b[0m end\x1b[3;7H";
    let (owner, follower) = followed(24, 80, output.as_bytes());

    assert_same(&owner, &follower);
    assert_eq!(follower.lock().grid().cursor.point, Point::new(Line(2), Column(6)));
    assert_eq!(rows_text(&follower.lock())[1], "wide \u{4f60} \u{597d}  and e");
    assert_eq!(
        follower.lock().grid()[Line(0)][Column(6)].fg,
        Color::Named(NamedColor::Red)
    );

    // The output after the screen draws the same on both sides.
    draw_on_both(&owner, &follower, b"X\x1b[K\r\n\x1b[32mnext\x1b[1;1H\x1b[2Ptop");
    assert_same(&owner, &follower);
}

#[test]
fn a_follower_has_every_mode_of_the_owner() {
    let modes: [(&str, TermMode); 22] = [
        ("\x1b[?1h", TermMode::APP_CURSOR),
        ("\x1b=", TermMode::APP_KEYPAD),
        ("\x1b[?2004h", TermMode::BRACKETED_PASTE),
        ("\x1b[?1000h", TermMode::MOUSE_REPORT_CLICK),
        ("\x1b[?1002h", TermMode::MOUSE_DRAG),
        ("\x1b[?1003h", TermMode::MOUSE_MOTION),
        ("\x1b[?1005h", TermMode::UTF8_MOUSE),
        ("\x1b[?1006h", TermMode::SGR_MOUSE),
        ("\x1b[?1004h", TermMode::FOCUS_IN_OUT),
        ("\x1b[?25l", TermMode::SHOW_CURSOR),
        ("\x1b[?1007l", TermMode::ALTERNATE_SCROLL),
        ("\x1b[?1042l", TermMode::URGENCY_HINTS),
        ("\x1b[?7l", TermMode::LINE_WRAP),
        ("\x1b[?6h", TermMode::ORIGIN),
        ("\x1b[4h", TermMode::INSERT),
        ("\x1b[20h", TermMode::LINE_FEED_NEW_LINE),
        ("\x1b[>1u", TermMode::DISAMBIGUATE_ESC_CODES),
        ("\x1b[>2u", TermMode::REPORT_EVENT_TYPES),
        ("\x1b[>4u", TermMode::REPORT_ALTERNATE_KEYS),
        ("\x1b[>8u", TermMode::REPORT_ALL_KEYS_AS_ESC),
        ("\x1b[>16u", TermMode::REPORT_ASSOCIATED_TEXT),
        ("\x1b[?1049h", TermMode::ALT_SCREEN),
    ];
    for (sequence, flag) in modes {
        let output = format!("first row\r\nsecond row{sequence}");
        let (owner, follower) = followed(6, 20, output.as_bytes());

        let changed = *owner.lock().mode() ^ TermMode::default();
        assert_eq!(changed, flag, "{sequence:?} changes one mode of the owner");
        assert_same(&owner, &follower);

        // The mode acts on the next output of the follower as on the owner.
        draw_on_both(&owner, &follower, b"X\nY and a row longer than the grid\x1b[2;3Hq");
        assert_same(&owner, &follower);
    }
}

#[test]
fn a_follower_has_the_modes_of_a_program_that_set_many() {
    let output = "\x1b[?1h\x1b=\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h\x1b[>31u\x1b[?25l\x1b[5 qtext";
    let (owner, follower) = followed(24, 80, output.as_bytes());

    assert_same(&owner, &follower);
    let mode = *follower.lock().mode();
    let wanted = TermMode::APP_CURSOR
        | TermMode::APP_KEYPAD
        | TermMode::MOUSE_DRAG
        | TermMode::SGR_MOUSE
        | TermMode::BRACKETED_PASTE
        | TermMode::FOCUS_IN_OUT
        | TermMode::KITTY_KEYBOARD_PROTOCOL;
    assert!(mode.contains(wanted), "the input modes of the program: {mode:?}");
    assert!(!mode.contains(TermMode::SHOW_CURSOR), "the hidden cursor");
    assert_eq!(follower.lock().cursor_style().shape, CursorShape::Beam);
    assert!(follower.lock().cursor_style().blinking);
}

#[test]
fn a_follower_of_the_alternate_screen_enters_it_and_draws_it() {
    let output = "the primary screen\r\n\x1b[?1049h\x1b[2J\x1b[Halternate \x1b[32mgreen\x1b[0m\
                  \x1b[10;5Hstatus\x1b[?1h\x1b[?1002h\x1b[?1006h\x1b[>1u\x1b[?25l\x1b[4;2H";
    let (owner, follower) = followed(12, 40, output.as_bytes());

    assert_same(&owner, &follower);
    assert!(follower.lock().mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(rows_text(&follower.lock())[0], "alternate green");
    assert_eq!(rows_text(&follower.lock())[9], "    status");

    draw_on_both(&owner, &follower, b"\x1b[2;1Hmore\x1b[?1003h");
    assert_same(&owner, &follower);
}

#[test]
fn a_follower_of_a_wide_grid_and_of_an_empty_grid_shows_the_same() {
    let mut output = String::from("\x1b[60;1Hlast row");
    for row in [1, 30, 59] {
        let _ = write!(
            output,
            "\x1b[{row};385H\x1b[35mfar right \u{754c}\x1b[0m\x1b[{row};1Hleft"
        );
    }
    // The cursor ends in the last cell of the grid and waits behind it.
    output.push_str("\x1b[60;393H12345678");
    let (owner, follower) = followed(60, 400, output.as_bytes());
    assert_same(&owner, &follower);
    assert_eq!(follower.lock().grid()[Line(29)][Column(394)].c, '\u{754c}');
    assert!(follower.lock().grid().cursor.input_needs_wrap);

    let (owner, follower) = followed(24, 80, b"");
    assert_same(&owner, &follower);
    assert_eq!(follower.lock().grid().cursor.point, Point::new(Line(0), Column(0)));
    assert!(rows_text(&follower.lock()).iter().all(String::is_empty));
}

#[test]
fn a_tab_and_hidden_text_keep_the_columns_of_their_row() {
    let (owner, follower) = followed(4, 40, b"a\tb\tc\r\nshown\x1b[8mhidden\x1b[0mafter");

    assert_eq!(
        owner.lock().grid()[Line(0)][Column(1)].c,
        '\t',
        "the grid keeps the tab"
    );
    let text = rows_text(&follower.lock());
    assert_eq!(text[0], "a       b       c");
    assert_eq!(text[1], "shown      after");
    assert_eq!(follower.lock().grid().cursor.point, owner.lock().grid().cursor.point);
}

#[test]
fn a_cursor_that_waits_behind_the_last_column_waits_there_for_the_follower() {
    let rows: [(&str, u16); 3] = [
        ("1234567890", 10),
        ("123456789\x1b[41m\u{754c}", 11),
        ("1234567890\x1b[K", 10),
    ];
    for (output, cols) in rows {
        let (owner, follower) = followed(3, cols, output.as_bytes());
        assert!(owner.lock().grid().cursor.input_needs_wrap, "{output:?} fills its row");
        assert_same(&owner, &follower);

        // The next character starts the next row on both sides.
        draw_on_both(&owner, &follower, b"Z");
        assert_same(&owner, &follower);
        assert_eq!(follower.lock().grid()[Line(1)][Column(0)].c, 'Z');
    }
}

#[test]
fn the_pen_of_the_owner_draws_the_next_output_of_the_follower() {
    let (owner, follower) = followed(4, 30, b"plain \x1b[1;3;35;46m");

    assert_same(&owner, &follower);
    draw_on_both(&owner, &follower, b"painted");
    assert_same(&owner, &follower);
    assert_eq!(
        follower.lock().grid()[Line(0)][Column(6)].fg,
        Color::Named(NamedColor::Magenta)
    );
}

#[test]
fn a_follower_that_showed_an_older_screen_shows_the_new_one_alone() {
    let follower = screen(8, 30);
    replay_live_bytes(
        &follower,
        b"old\r\nrows\x1b[?1049h\x1b[?1h\x1b[?1003h\x1b[>31u\x1b[2;5r\x1b[4hold alternate\x1b[3 q\x1b[31m",
    );

    let owner = screen(8, 30);
    replay_live_bytes(&owner, b"new\x1b[?2004h");
    follow(&owner, &follower);

    assert_same(&owner, &follower);
    assert_eq!(rows_text(&follower.lock())[..2], ["new", ""]);
    // The scroll region of the older screen is gone too.
    draw_on_both(&owner, &follower, b"\x1b[8;1H\r\nscrolled");
    assert_same(&owner, &follower);
}

#[test]
fn an_owner_that_reads_its_history_shares_the_screen_of_the_program() {
    let owner = screen(4, 20);
    let lines: String = (1..=30).fold(String::new(), |mut lines, line| {
        let _ = write!(lines, "line {line}\r\n");
        lines
    });
    replay_live_bytes(&owner, lines.as_bytes());
    owner.lock().scroll_display(Scroll::Delta(10));

    let follower = screen(4, 20);
    follow(&owner, &follower);

    assert_eq!(rows_text(&follower.lock()), ["line 28", "line 29", "line 30", ""]);
}

mod program;
