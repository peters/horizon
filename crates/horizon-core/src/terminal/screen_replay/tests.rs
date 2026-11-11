use std::sync::Arc;
use std::sync::mpsc;

use alacritty_terminal::event::Event;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{self, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use super::screen_replay_bytes;
use crate::terminal::support::replay_terminal_bytes;
use crate::terminal::{TerminalDimensions, TerminalEventProxy, TerminalSshTrust};

fn screen(rows: u16, cols: u16) -> Arc<FairMutex<Term<TerminalEventProxy>>> {
    let (event_tx, _event_rx) = mpsc::channel::<Event>();
    let dimensions = TerminalDimensions::new(rows, cols);
    Arc::new(FairMutex::new(Term::new(
        term::Config::default(),
        &dimensions,
        TerminalEventProxy::new(event_tx, TerminalSshTrust::default()),
    )))
}

/// The text that a terminal shows after it replays `replay`, row by row.
fn lines(replay: &[u8]) -> Vec<String> {
    let passed = screen(24, 80);
    replay_terminal_bytes(&passed, replay);
    let mut lines = Vec::new();
    {
        let term = passed.lock();
        let grid = term.grid();
        for row in 0..grid.screen_lines() {
            let line = &grid[Line(i32::try_from(row).expect("a row of the screen"))];
            let text: String = line[..Column(grid.columns())].iter().map(|cell| cell.c).collect();
            lines.push(text.trim_end().to_owned());
        }
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

#[test]
fn the_replay_draws_the_rows_of_the_screen() {
    let old = screen(24, 80);
    replay_terminal_bytes(&old, b"one\r\ntwo\r\nthree");

    let replay = screen_replay_bytes(&old.lock());

    assert_eq!(lines(&replay), ["one", "two", "three"]);
}

#[test]
fn the_replay_keeps_the_colour_of_a_run_and_the_default_after_it() {
    let old = screen(24, 80);
    replay_terminal_bytes(&old, b"\x1b[31mred\x1b[0m plain");

    let replay = screen_replay_bytes(&old.lock());
    let passed = screen(24, 80);
    replay_terminal_bytes(&passed, &replay);

    let term = passed.lock();
    let line = &term.grid()[Line(0)];
    assert_eq!(line[Column(0)].c, 'r');
    assert_eq!(
        line[Column(0)].fg,
        Color::Named(NamedColor::Red),
        "the red run is still red"
    );
    assert_eq!(
        line[Column(4)].fg,
        Color::Named(NamedColor::Foreground),
        "the text after the run is default again"
    );
}

#[test]
fn the_replay_ends_at_the_content_and_lifts_no_row_into_scrollback() {
    let old = screen(24, 80);
    replay_terminal_bytes(&old, b"kept");

    let replay = screen_replay_bytes(&old.lock());
    let passed = screen(24, 80);
    replay_terminal_bytes(&passed, &replay);

    let term = passed.lock();
    assert_eq!(term.grid()[Line(0)][Column(0)].c, 'k', "the first row keeps the text");
    assert!(
        !term.mode().contains(TermMode::ALT_SCREEN),
        "the replay enters no screen"
    );
}

#[test]
fn the_new_output_of_a_connection_replaces_the_replayed_screen() {
    let old = screen(24, 80);
    replay_terminal_bytes(&old, b"lost-link");
    let replay = screen_replay_bytes(&old.lock());

    let passed = screen(24, 80);
    replay_terminal_bytes(&passed, &replay);
    assert_eq!(
        lines(&replay),
        ["lost-link"],
        "the replayed screen shows before the link"
    );

    replay_terminal_bytes(&passed, b"\x1b[H\x1b[2Jagain");
    let mut rows = Vec::new();
    {
        let term = passed.lock();
        let grid = term.grid();
        for row in 0..2 {
            let line = &grid[Line(row)];
            rows.push(line[..Column(5)].iter().map(|cell| cell.c).collect::<String>());
        }
    }
    assert_eq!(rows[0], "again", "the first output of the connection draws over it");
    assert_eq!(rows[1], "     ", "and no row of the lost screen is left");
}

#[test]
fn an_empty_screen_replays_no_bytes() {
    let old = screen(24, 80);

    let replay = screen_replay_bytes(&old.lock());

    assert!(replay.is_empty(), "an empty screen draws nothing: {replay:?}");
}
