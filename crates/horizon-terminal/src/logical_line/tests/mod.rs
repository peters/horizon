mod guards;
mod joins;

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::{self, Term, viewport_to_point};
use alacritty_terminal::vte::ansi;

use super::{LogicalLine, RowJoin, logical_line_at};
use crate::links::find_url_at_column;
use crate::test_size::TestSize;

const COLS: usize = 40;
const OAUTH_URL: &str = "https://login.example.com/oauth/authorize?client_id=0123456789abcdef\
    &redirect_uri=https%3A%2F%2Fexample.org%2Fcallback&scope=read+write&state=Zm9vYmFy";

fn term_with_rows(cols: usize, screen_rows: usize, rows: &[String]) -> Term<VoidListener> {
    let config = term::Config {
        scrolling_history: 64,
        ..term::Config::default()
    };
    let mut term = Term::new(config, &TestSize::new(screen_rows, cols), VoidListener);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    for (index, row) in rows.iter().enumerate() {
        assert!(row.chars().count() <= cols, "row {index} overflows: {row:?}");
        if index > 0 {
            parser.advance(&mut term, b"\r\n");
        }
        parser.advance(&mut term, row.as_bytes());
    }
    term
}

/// Break `text` the way a wrapping program does: the first row starts
/// with `first_prefix`, later rows with `prefix`, and every row holds at
/// most `width` columns of `text`.
fn hard_wrap(text: &str, first_prefix: &str, prefix: &str, width: usize, first_width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let (first, rest) = chars.split_at(first_width.min(chars.len()));
    let mut rows = vec![format!("{first_prefix}{}", first.iter().collect::<String>())];
    rows.extend(
        rest.chunks(width)
            .map(|chunk| format!("{prefix}{}", chunk.iter().collect::<String>())),
    );
    rows
}

/// The logical line at a viewport cell, as a panel that draws the grid asks for it.
fn logical_line_at_viewport_point(
    term: &Term<VoidListener>,
    cols: usize,
    row: usize,
    col: usize,
    join: RowJoin,
) -> Option<LogicalLine> {
    assert_eq!(cols, term.columns(), "the test term has {cols} columns");
    let point = viewport_to_point(term.grid().display_offset(), Point::new(row, Column(col)));
    logical_line_at(term, point, join)
}

fn url_at(term: &Term<VoidListener>, cols: usize, row: usize, col: usize) -> Option<String> {
    let line = logical_line_at_viewport_point(term, cols, row, col, RowJoin::UrlHardWraps)?;
    find_url_at_column(&line.chars, line.column)
}

fn texts(rows: &[&str]) -> Vec<String> {
    rows.iter().map(ToString::to_string).collect()
}
