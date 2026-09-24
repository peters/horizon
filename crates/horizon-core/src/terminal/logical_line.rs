//! Logical text lines assembled from terminal rows for click-target detection.
//!
//! Rows the terminal soft-wrapped always join. Programs that wrap their own
//! output (agent TUIs) and multiplexers that redraw it (tmux) leave hard line
//! breaks through the middle of long URLs instead. [`RowJoin::UrlHardWraps`]
//! also joins those rows when their shape matches a URL broken across rows.

use std::ops::Range;

use alacritty_terminal::grid::{Dimensions, Grid, Row};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, viewport_to_point};

use super::support::starts_with_url_scheme;

/// Hard line breaks followed from the clicked row in each direction.
const MAX_HARD_WRAPPED_ROWS: usize = 64;
/// Blank columns a wrapping program may leave after a URL segment that fills
/// its row, such as a message background that stops one column short.
const MAX_WRAP_PADDING: usize = 1;
/// Characters after which line breakers split URLs with a ragged right edge.
const URL_BREAK_CHARS: [char; 3] = ['/', '-', '?'];
/// Delimiters that mark continuation text as part of a URL rather than prose.
const URL_DELIMITERS: [char; 6] = ['/', '?', '#', '&', '=', '%'];
/// Characters that join words inside a URL path segment.
const URL_WORD_JOINERS: [char; 3] = ['-', '_', '.'];
const SENTENCE_PUNCTUATION: [char; 5] = ['.', ',', ';', ':', '!'];
const PROMPT_TERMINATORS: [char; 2] = ['$', '#'];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowJoin {
    /// Join only rows the terminal soft-wrapped.
    SoftWraps,
    /// Also join rows a program hard-wrapped through the middle of a URL.
    UrlHardWraps,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Joint {
    Soft,
    Hard,
}

pub(super) struct LogicalLine {
    pub(super) chars: Vec<char>,
    /// Index of the clicked cell within `chars`.
    pub(super) column: usize,
    /// Whether any hard-wrapped rows were joined, so the text differs from
    /// the [`RowJoin::SoftWraps`] line at the same cell.
    pub(super) joins_hard_wraps: bool,
}

/// Return the logical line containing a viewport cell. Hard-wrap joints drop
/// the continuation row's indentation and the previous row's trailing
/// padding, so a click on those blank cells has no logical position.
pub(super) fn logical_line_at_viewport_point<T>(
    term: &Term<T>,
    cols: usize,
    row: usize,
    col: usize,
    join: RowJoin,
) -> Option<LogicalLine> {
    let grid = term.grid();
    if col >= cols || row >= grid.screen_lines() {
        return None;
    }

    let clicked = viewport_to_point(grid.display_offset(), Point::new(row, Column(col))).line;
    let (first, joints) = logical_line_rows(grid, cols, clicked, join);

    let mut chars = Vec::with_capacity(cols * (joints.len() + 1));
    let mut column = None;
    let mut line = first;
    for index in 0..=joints.len() {
        let cells = &grid[line];
        let start = if index > 0 && joints[index - 1] == Joint::Hard {
            first_content_column(cells, cols).unwrap_or(0)
        } else {
            0
        };
        let end = if joints.get(index) == Some(&Joint::Hard) {
            last_content_column(cells, cols).map_or(cols, |column| column + 1)
        } else {
            cols
        };

        if line == clicked {
            if !(start..end).contains(&col) {
                return None;
            }
            column = Some(chars.len() + col - start);
        }
        chars.extend(row_chars(cells, start..end));
        line += 1;
    }

    Some(LogicalLine {
        chars,
        column: column?,
        joins_hard_wraps: joints.contains(&Joint::Hard),
    })
}

/// The first row of the logical line around `clicked` and, in row order, how
/// each of its rows continues onto the next.
fn logical_line_rows(grid: &Grid<Cell>, cols: usize, clicked: Line, join: RowJoin) -> (Line, Vec<Joint>) {
    let mut joints_above = Vec::new();
    let mut first = clicked;
    let mut hard_joints = 0;
    while first > grid.topmost_line() {
        match joint_after(grid, cols, first - 1, join) {
            Some(Joint::Soft) => joints_above.push(Joint::Soft),
            Some(Joint::Hard) if hard_joints < MAX_HARD_WRAPPED_ROWS => {
                hard_joints += 1;
                joints_above.push(Joint::Hard);
            }
            _ => break,
        }
        first -= 1;
    }

    let mut joints = joints_above;
    joints.reverse();
    let mut last = clicked;
    hard_joints = 0;
    while last < grid.bottommost_line() {
        match joint_after(grid, cols, last, join) {
            Some(Joint::Soft) => joints.push(Joint::Soft),
            Some(Joint::Hard) if hard_joints < MAX_HARD_WRAPPED_ROWS => {
                hard_joints += 1;
                joints.push(Joint::Hard);
            }
            _ => break,
        }
        last += 1;
    }

    (first, joints)
}

/// How `line` continues onto the row below it, if at all. The caller keeps
/// `line` above the grid's bottommost line.
fn joint_after(grid: &Grid<Cell>, cols: usize, line: Line, join: RowJoin) -> Option<Joint> {
    let upper = &grid[line];
    if upper[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
        return Some(Joint::Soft);
    }
    (join == RowJoin::UrlHardWraps && url_continues_on_next_row(upper, &grid[line + 1], cols)).then_some(Joint::Hard)
}

/// Whether `upper` ends with a URL segment that a program broke onto `lower`.
///
/// The continuation must start no further right than the segment and must
/// not look like a new URL, a list item or a shell prompt. A segment that
/// fills its row up to the right edge (give or take a padding column) and is
/// the row's only content continues onto URL-delimited text or onto a row
/// holding nothing but one word, which may be a letters-only URL tail but not
/// a sentence. After other words, word wrappers only break a URL longer than
/// a row, so the continuation must hold URL delimiters and be too long to
/// have fit on the segment's row. Line breakers that split URLs after
/// punctuation leave a ragged edge instead; those rows join only when the
/// continuation row is a single URL-shaped word.
fn url_continues_on_next_row(upper: &Row<Cell>, lower: &Row<Cell>, cols: usize) -> bool {
    let (Some(upper_start), Some(upper_end), Some(lower_start), Some(lower_end)) = (
        first_content_column(upper, cols),
        last_content_column(upper, cols),
        first_content_column(lower, cols),
        last_content_column(lower, cols),
    ) else {
        return false;
    };
    let Some(segment_start) = url_segment_start(upper, upper_end) else {
        return false;
    };
    let continuation = lower_start..url_run_end(lower, lower_start, cols);
    let continuation_len = continuation.len();
    if lower_start > segment_start
        || continuation.is_empty()
        || !starts_url_continuation(lower[Column(lower_start)].c)
        || starts_with_url_scheme(&row_chars(lower, continuation.clone()))
        || starts_list_item_or_prompt(lower, continuation.clone(), cols)
    {
        return false;
    }

    let segment = segment_start..upper_end + 1;
    let segment_is_row_content = only_marker_before(upper, upper_start, segment_start);
    let continuation_is_row_content = continuation.end == lower_end + 1;
    let continuation_ends_sentence = SENTENCE_PUNCTUATION.contains(&lower[Column(continuation.end - 1)].c);
    let continuation_has_delimiters =
        row_chars(lower, continuation.clone()).any(|character| URL_DELIMITERS.contains(&character));
    if cols - 1 - upper_end <= MAX_WRAP_PADDING {
        return if segment_is_row_content {
            continuation_has_delimiters || (continuation_is_row_content && !continuation_ends_sentence)
        } else {
            continuation_has_delimiters && segment.len() + continuation_len > cols - lower_start
        };
    }

    (segment_is_row_content || contains_scheme_separator(row_chars(upper, segment)))
        && URL_BREAK_CHARS.contains(&upper[Column(upper_end)].c)
        && continuation_is_row_content
        && (continuation_has_delimiters
            || row_chars(lower, continuation).any(|character| URL_WORD_JOINERS.contains(&character)))
        && !continuation_ends_sentence
}

/// Whether the row's first word is a list marker (`-`, `*`, `12.`) or ends a
/// shell prompt (`user@host:~$`) rather than continuing a URL.
fn starts_list_item_or_prompt(row: &Row<Cell>, word: Range<usize>, cols: usize) -> bool {
    let followed_by_blank = word.end < cols && is_blank(&row[Column(word.end)]);
    followed_by_blank
        && (is_marker(row_chars(row, word.clone())) || PROMPT_TERMINATORS.contains(&row[Column(word.end - 1)].c))
}

/// Start of the run of URL characters ending at `end`, if `end` holds one.
fn url_segment_start(row: &Row<Cell>, end: usize) -> Option<usize> {
    if !is_url_char(row[Column(end)].c) {
        return None;
    }
    let mut start = end;
    while start > 0 && is_url_char(row[Column(start - 1)].c) {
        start -= 1;
    }
    Some(start)
}

/// End (exclusive) of the run of URL characters starting at `start`.
fn url_run_end(row: &Row<Cell>, start: usize, cols: usize) -> usize {
    (start..cols)
        .find(|&column| !is_url_char(row[Column(column)].c))
        .unwrap_or(cols)
}

/// Whether the cells between the row's content start and `segment_start`
/// hold only a list or prompt marker followed by blanks.
fn only_marker_before(row: &Row<Cell>, content_start: usize, segment_start: usize) -> bool {
    if content_start == segment_start {
        return true;
    }
    let marker_end = (content_start..segment_start)
        .find(|&column| is_blank(&row[Column(column)]))
        .unwrap_or(segment_start);
    (marker_end..segment_start).all(|column| is_blank(&row[Column(column)]))
        && is_marker(row_chars(row, content_start..marker_end))
}

/// A bullet or prompt glyph (`•`, `❯`, `-`, `>`) or an ordered-list number
/// (`1.`, `12)`).
fn is_marker(mut chars: impl Iterator<Item = char>) -> bool {
    match chars.next() {
        Some(first) if !first.is_ascii_alphanumeric() => chars.next().is_none(),
        Some(first) if first.is_ascii_digit() => {
            let mut digits = 1;
            while let Some(character) = chars.next() {
                if character.is_ascii_digit() && digits < 3 {
                    digits += 1;
                } else {
                    return matches!(character, '.' | ')') && chars.next().is_none();
                }
            }
            false
        }
        _ => false,
    }
}

fn contains_scheme_separator(chars: impl Iterator<Item = char>) -> bool {
    let mut previous = ['\0'; 2];
    for character in chars {
        if previous == [':', '/'] && character == '/' {
            return true;
        }
        previous = [previous[1], character];
    }
    false
}

fn row_chars(row: &Row<Cell>, columns: Range<usize>) -> impl Iterator<Item = char> + Clone + '_ {
    columns.map(move |column| row[Column(column)].c)
}

fn first_content_column(row: &Row<Cell>, cols: usize) -> Option<usize> {
    (0..cols).find(|&column| !is_blank(&row[Column(column)]))
}

fn last_content_column(row: &Row<Cell>, cols: usize) -> Option<usize> {
    (0..cols).rev().find(|&column| !is_blank(&row[Column(column)]))
}

fn is_blank(cell: &Cell) -> bool {
    matches!(cell.c, ' ' | '\t')
}

/// ASCII characters RFC 3986 allows in a URI, including percent escapes. The
/// apostrophe is excluded because URL detection already ends a URL there.
fn is_url_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || "-._~:/?#[]@!$&()*+,;=%".contains(character)
}

/// Rows opening with a bracket are status lines, log prefixes and asides, such
/// as tmux's status bar directly below a pane, far more often than URL text.
fn starts_url_continuation(character: char) -> bool {
    !matches!(character, '[' | '(') && is_url_char(character)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::term::{self, Term};
    use alacritty_terminal::vte::ansi;

    use super::{RowJoin, logical_line_at_viewport_point};
    use crate::terminal::{TerminalDimensions, TerminalEventProxy, TerminalSshTrust, find_url_at_column};

    const COLS: usize = 40;
    const OAUTH_URL: &str = "https://login.example.com/oauth/authorize?client_id=0123456789abcdef\
        &redirect_uri=https%3A%2F%2Fexample.org%2Fcallback&scope=read+write&state=Zm9vYmFy";

    fn term_with_rows(cols: usize, screen_rows: usize, rows: &[String]) -> Term<TerminalEventProxy> {
        let (event_tx, _event_rx) = mpsc::channel();
        let dimensions = TerminalDimensions::new(
            u16::try_from(screen_rows).expect("rows fit"),
            u16::try_from(cols).expect("cols fit"),
        );
        let config = term::Config {
            scrolling_history: 64,
            ..term::Config::default()
        };
        let mut term = Term::new(
            config,
            &dimensions,
            TerminalEventProxy::new(event_tx, TerminalSshTrust::default()),
        );
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

    fn url_at(term: &Term<TerminalEventProxy>, cols: usize, row: usize, col: usize) -> Option<String> {
        let line = logical_line_at_viewport_point(term, cols, row, col, RowJoin::UrlHardWraps)?;
        find_url_at_column(&line.chars, line.column)
    }

    fn texts(rows: &[&str]) -> Vec<String> {
        rows.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn full_width_hard_wraps_join_from_every_row() {
        let mut rows = texts(&["Use the url below to sign in:", ""]);
        let url_rows = hard_wrap(OAUTH_URL, "", "", COLS, COLS);
        let last_url_row = rows.len() + url_rows.len() - 1;
        rows.extend(url_rows);
        rows.extend(texts(&["", "Paste code here >"]));
        let term = term_with_rows(COLS, 12, &rows);

        for (row, col) in [(2, 0), (2, COLS - 1), (3, 7), (last_url_row, 0)] {
            assert_eq!(
                url_at(&term, COLS, row, col).as_deref(),
                Some(OAUTH_URL),
                "row {row} col {col}"
            );
        }
        assert_eq!(url_at(&term, COLS, last_url_row, COLS - 1), None);
        assert_eq!(url_at(&term, COLS, 0, 4), None);
    }

    #[test]
    fn indented_first_row_joins_flush_continuation_rows() {
        let term = term_with_rows(COLS, 8, &hard_wrap(OAUTH_URL, "  ", "", COLS, COLS - 2));

        assert_eq!(url_at(&term, COLS, 0, 2).as_deref(), Some(OAUTH_URL));
        assert_eq!(url_at(&term, COLS, 2, 10).as_deref(), Some(OAUTH_URL));
    }

    #[test]
    fn indented_rows_with_trailing_padding_join() {
        let rows: Vec<String> = hard_wrap(OAUTH_URL, "  ", "  ", COLS - 3, COLS - 3)
            .into_iter()
            .map(|row| format!("{row:<width$}", width = COLS - 1))
            .collect();
        let term = term_with_rows(COLS, 8, &rows);

        assert_eq!(url_at(&term, COLS, 0, 5).as_deref(), Some(OAUTH_URL));
        assert_eq!(url_at(&term, COLS, 3, 2).as_deref(), Some(OAUTH_URL));
        assert_eq!(
            url_at(&term, COLS, 1, 0),
            None,
            "continuation indentation is not part of the URL"
        );
        assert_eq!(
            url_at(&term, COLS, 1, COLS - 1),
            None,
            "trailing padding is not part of the URL"
        );
    }

    #[test]
    fn bullet_marker_row_joins_indented_continuation() {
        let term = term_with_rows(COLS, 8, &hard_wrap(OAUTH_URL, "\u{25cf} ", "  ", COLS - 2, COLS - 2));

        assert_eq!(url_at(&term, COLS, 0, 2).as_deref(), Some(OAUTH_URL));
        assert_eq!(url_at(&term, COLS, 2, 4).as_deref(), Some(OAUTH_URL));
    }

    #[test]
    fn ragged_breaks_after_url_punctuation_join() {
        let cols = 60;
        let url = "https://docs.example.com/guides/long-links?topic=terminal-rendering&section=query-strings-\
            wrapping&ref=test-0001";
        let term = term_with_rows(
            cols,
            8,
            &texts(&[
                "     Here is the link:",
                "",
                "     https://docs.example.com/guides/long-links?",
                "     topic=terminal-rendering&section=query-strings-",
                "     wrapping&ref=test-0001",
                "",
                "     That link wraps twice.",
            ]),
        );

        for row in 2..=4 {
            assert_eq!(url_at(&term, cols, row, 8).as_deref(), Some(url), "row {row}");
        }
        assert_eq!(url_at(&term, cols, 6, 8), None);
    }

    #[test]
    fn url_ending_short_of_the_edge_does_not_absorb_the_next_line() {
        let cols = 60;
        let url = "https://example.com/docs/desktop-app";
        let first = format!("  Tip: install it from {url}");
        assert_eq!(first.chars().count(), cols - 1);
        let term = term_with_rows(cols, 4, &[first, "  and run it.".to_string()]);

        assert_eq!(url_at(&term, cols, 0, 30).as_deref(), Some(url));
        assert_eq!(url_at(&term, cols, 1, 3), None);
    }

    #[test]
    fn url_after_prose_that_exactly_fills_the_row_keeps_its_end() {
        let cols = 60;
        let url = "https://example.com/docs/desktop-apps";
        let first = format!("  Tip: install it from {url}");
        assert_eq!(first.chars().count(), cols);
        for next in ["  and run it.", "  for details.", "  - next item", "  2. next item"] {
            let term = term_with_rows(cols, 4, &[first.clone(), next.to_string()]);

            assert_eq!(url_at(&term, cols, 0, 30).as_deref(), Some(url), "next row {next:?}");
            assert_eq!(url_at(&term, cols, 1, 3), None, "next row {next:?}");
        }
    }

    #[test]
    fn row_filling_url_does_not_absorb_the_next_sentence() {
        let first = format!("https://example.com/{}", "a".repeat(COLS - 20));
        for next in ["Thanks for reading.", "Thanks.", "see the docs for more"] {
            let term = term_with_rows(COLS, 4, &[first.clone(), next.to_string()]);

            assert_eq!(url_at(&term, COLS, 0, 5), Some(first.clone()), "next row {next:?}");
            assert_eq!(url_at(&term, COLS, 1, 2), None, "next row {next:?}");
        }
    }

    #[test]
    fn row_filling_url_joins_a_letters_only_tail() {
        let first = format!("https://example.com/state={}", "a".repeat(COLS - 26));
        let term = term_with_rows(COLS, 4, &[first.clone(), "Yrc".to_string(), String::new()]);

        assert_eq!(url_at(&term, COLS, 1, 1), Some(format!("{first}Yrc")));
    }

    #[test]
    fn url_after_prose_joins_a_continuation_of_url_text() {
        let rows: Vec<String> = hard_wrap(OAUTH_URL, "  fix this ", "  ", COLS - 3, COLS - 12)
            .into_iter()
            .map(|row| format!("{row:<width$}", width = COLS - 1))
            .collect();
        let term = term_with_rows(COLS, 8, &rows);

        assert_eq!(url_at(&term, COLS, 0, 20).as_deref(), Some(OAUTH_URL));
        assert_eq!(url_at(&term, COLS, 1, 5).as_deref(), Some(OAUTH_URL));
    }

    #[test]
    fn list_item_url_joins_its_continuation_but_not_the_next_item() {
        let term = term_with_rows(COLS, 8, &hard_wrap(OAUTH_URL, "- ", "  ", COLS - 2, COLS - 2));
        assert_eq!(url_at(&term, COLS, 1, 5).as_deref(), Some(OAUTH_URL));

        let item = format!("- https://a.example/{}", "a".repeat(COLS - 20));
        let term = term_with_rows(COLS, 4, &[item.clone(), "- next item".to_string()]);
        assert_eq!(url_at(&term, COLS, 0, 5).as_deref(), Some(&item[2..]));
    }

    #[test]
    fn shell_prompt_after_a_url_is_not_a_continuation() {
        let term = term_with_rows(COLS, 4, &texts(&["https://example.com/api/", "peters@host:~/work$"]));
        assert_eq!(url_at(&term, COLS, 0, 5).as_deref(), Some("https://example.com/api/"));

        let first = format!("https://a.example/{}", "a".repeat(COLS - 18));
        let term = term_with_rows(COLS, 4, &[first.clone(), "root@box:/workspace# ls".to_string()]);
        assert_eq!(url_at(&term, COLS, 0, 5), Some(first));
    }

    #[test]
    fn ragged_break_inside_a_path_segment_joins() {
        let cols = 60;
        let term = term_with_rows(
            cols,
            4,
            &texts(&["     https://docs.example.com/guides/some-long-", "     file-name.md"]),
        );

        assert_eq!(
            url_at(&term, cols, 1, 8).as_deref(),
            Some("https://docs.example.com/guides/some-long-file-name.md")
        );
    }

    #[test]
    fn prose_after_a_ragged_url_is_not_joined() {
        let cols = 60;
        for next in ["  Thanks.", "  for more details", "  guides"] {
            let term = term_with_rows(cols, 4, &texts(&["  Docs live at https://example.com/guides/", next]));

            assert_eq!(
                url_at(&term, cols, 0, 20).as_deref(),
                Some("https://example.com/guides/"),
                "next row {next:?}"
            );
        }
    }

    #[test]
    fn a_new_url_on_the_next_row_starts_a_new_line() {
        let first = format!("https://a.example/{}", "a".repeat(COLS - 18));
        let term = term_with_rows(COLS, 4, &[first.clone(), "https://b.example/next".to_string()]);

        assert_eq!(url_at(&term, COLS, 0, 5), Some(first));
        assert_eq!(url_at(&term, COLS, 1, 5).as_deref(), Some("https://b.example/next"));
    }

    #[test]
    fn status_line_below_a_full_url_row_is_not_a_continuation() {
        let first = format!("https://a.example/{}", "a".repeat(COLS - 18));
        let term = term_with_rows(COLS, 4, &[first.clone(), "[main] 0:bash*".to_string()]);

        assert_eq!(url_at(&term, COLS, 0, 5), Some(first));
    }

    #[test]
    fn deeper_indented_row_is_not_a_continuation() {
        let first = format!("https://a.example/{}", "a".repeat(COLS - 18));
        let term = term_with_rows(COLS, 4, &[first.clone(), "    continued".to_string()]);

        assert_eq!(url_at(&term, COLS, 0, 5), Some(first));
        assert_eq!(url_at(&term, COLS, 1, 6), None);
    }

    #[test]
    fn hard_wrapped_url_joins_rows_in_scrollback() {
        let mut rows = hard_wrap(OAUTH_URL, "", "", COLS, COLS);
        let url_rows = rows.len();
        rows.extend(texts(&["after-1", "after-2"]));
        let mut term = term_with_rows(COLS, 3, &rows);

        // Only the last URL row remains on screen; the rest is history.
        assert_eq!(url_at(&term, COLS, 0, 0).as_deref(), Some(OAUTH_URL));

        term.scroll_display(Scroll::Delta(i32::try_from(url_rows - 1).expect("rows fit")));
        assert_eq!(url_at(&term, COLS, 0, 3).as_deref(), Some(OAUTH_URL));
    }

    #[test]
    fn soft_wrap_join_ignores_hard_wrapped_rows() {
        let term = term_with_rows(COLS, 8, &hard_wrap(OAUTH_URL, "", "", COLS, COLS));

        let line = logical_line_at_viewport_point(&term, COLS, 1, 3, RowJoin::SoftWraps).expect("row is on screen");
        assert_eq!(line.chars.len(), COLS);
        assert_eq!(line.column, 3);
        assert!(!line.joins_hard_wraps);
    }
}
