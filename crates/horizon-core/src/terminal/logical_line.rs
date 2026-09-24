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

/// Hard line breaks followed from the clicked row in each direction.
const MAX_HARD_WRAPPED_ROWS: usize = 64;
/// Blank columns a wrapping program may leave after a URL segment that fills
/// its row, such as a message background that stops one column short.
const MAX_WRAP_PADDING: usize = 1;
/// Characters after which line breakers split URLs with a ragged right edge.
const URL_BREAK_CHARS: [char; 3] = ['/', '-', '?'];
/// Delimiters that mark continuation text as part of a URL rather than prose.
const URL_DELIMITERS: [char; 6] = ['/', '?', '#', '&', '=', '%'];
/// Characters that open a query or fragment, the only URL evidence a
/// path-shaped row can give: file names may contain `&`, `=` or `%` too.
const URL_QUERY_DELIMITERS: [char; 2] = ['?', '#'];
/// Rows followed away from a joint for URL context: the query syntax below a
/// path-shaped row, or the scheme and open delimiters above a segment. It
/// matches the hard-wrap limit so that context reaches as far as a join does.
const MAX_URL_CONTEXT_ROWS: usize = MAX_HARD_WRAPPED_ROWS;
/// Characters that join words inside a URL path segment.
const URL_WORD_JOINERS: [char; 3] = ['-', '_', '.'];
const SENTENCE_PUNCTUATION: [char; 5] = ['.', ',', ';', ':', '!'];
const DELIMITER_PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
/// Characters that end sh (`$`), root (`#`) and zsh/csh (`%`) prompts.
const PROMPT_TERMINATORS: [char; 3] = ['$', '#', '%'];

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
    if grid[line][Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
        return Some(Joint::Soft);
    }
    (join == RowJoin::UrlHardWraps && url_continues_on_next_row(grid, cols, line)).then_some(Joint::Hard)
}

/// Whether row `line` ends with a URL segment that a program broke onto the
/// row below it.
///
/// The continuation must pass [`continuation_word`]. A segment that fills its
/// row up to the right edge (give or take a padding column) and is the row's
/// only content continues onto URL-delimited text or onto a row holding
/// nothing but one word, which may be a letters-only URL tail but not a
/// sentence. After other words, word wrappers only break a URL longer than a
/// row, so the continuation must hold URL delimiters and be too long to have
/// fit on the segment's row. Line breakers that split URLs after punctuation
/// leave a ragged edge instead; those rows join only when the continuation
/// row is a single URL-shaped word. A continuation that starts like a file
/// path counts as URL text only when it leads to query syntax, see
/// [`path_row_reaches_query`], or when it is the single word on its row and
/// continues a `file://` URL, so a path printed below a web URL stays
/// clickable on its own.
fn url_continues_on_next_row(grid: &Grid<Cell>, cols: usize, line: Line) -> bool {
    let (upper, lower) = (&grid[line], &grid[line + 1]);
    let (Some(upper_start), Some(upper_end), Some(lower_end)) = (
        first_content_column(upper, cols),
        last_content_column(upper, cols),
        last_content_column(lower, cols),
    ) else {
        return false;
    };
    let Some(segment_start) = url_segment_start(upper, upper_end) else {
        return false;
    };
    let Some(continuation) = continuation_word(lower, segment_start, cols) else {
        return false;
    };

    let segment = segment_start..upper_end + 1;
    let segment_is_row_content = only_marker_before(upper, upper_start, segment_start);
    let continuation_is_path = starts_path(row_chars(lower, continuation.clone()));
    let continuation_is_row_content = continuation.end == lower_end + 1;
    let segment_text = || segment_text_with_rows_above(grid, cols, line, segment_start);
    let continuation_ends_sentence =
        word_ends_sentence(lower, &continuation, cols, || unmatched_openers(segment_text().chars()));
    // A sentence's closing punctuation, such as the `?` of `Continue?`, is no
    // evidence of URL syntax.
    let continuation_body = continuation.start..continuation.end - usize::from(continuation_ends_sentence);
    let continuation_has_delimiters = if continuation_is_path {
        path_row_reaches_query(grid, cols, line + 1)
            || (continuation_is_row_content && !continuation_ends_sentence && segment_text().starts_with("file://"))
    } else {
        row_chars(lower, continuation_body).any(|character| URL_DELIMITERS.contains(&character))
    };
    let continuation_is_url_word = !continuation_is_path && continuation_is_row_content && !continuation_ends_sentence;
    if cols - 1 - upper_end <= MAX_WRAP_PADDING {
        return if segment_is_row_content {
            continuation_has_delimiters || continuation_is_url_word
        } else {
            continuation_has_delimiters && segment.len() + continuation.len() > cols - continuation.start
        };
    }

    (segment_is_row_content || contains_scheme_separator(row_chars(upper, segment)))
        && URL_BREAK_CHARS.contains(&upper[Column(upper_end)].c)
        && continuation_is_row_content
        && !continuation_ends_sentence
        && (continuation_has_delimiters
            || (continuation_is_url_word
                && row_chars(lower, continuation).any(|character| URL_WORD_JOINERS.contains(&character))))
}

/// The first word of `row` when it can continue a URL segment that starts at
/// `segment_start` on the row above: it starts no further right, with a URL
/// character, and is not a new URL, a list marker or a shell prompt.
fn continuation_word(row: &Row<Cell>, segment_start: usize, cols: usize) -> Option<Range<usize>> {
    let start = first_content_column(row, cols)?;
    let word = start..url_run_end(row, start, cols);
    (start <= segment_start
        && !word.is_empty()
        && starts_url_continuation(row[Column(start)].c)
        && !starts_with_scheme(row_chars(row, word.clone()))
        && !starts_list_item_or_prompt(row, word.clone(), cols))
    .then_some(word)
}

/// Whether a row that starts like a file path is URL text: its first word has
/// query or fragment syntax, or it fills its row and wraps onto rows that
/// reach such syntax within [`MAX_URL_CONTEXT_ROWS`] rows.
fn path_row_reaches_query(grid: &Grid<Cell>, cols: usize, mut line: Line) -> bool {
    for _ in 0..MAX_URL_CONTEXT_ROWS {
        let row = &grid[line];
        let (Some(start), Some(end)) = (first_content_column(row, cols), last_content_column(row, cols)) else {
            return false;
        };
        let word = start..url_run_end(row, start, cols);
        if row_chars(row, word.clone()).any(|character| URL_QUERY_DELIMITERS.contains(&character)) {
            return true;
        }
        let fills_row = word.end == end + 1 && cols - 1 - end <= MAX_WRAP_PADDING;
        if !fills_row || line >= grid.bottommost_line() || continuation_word(&grid[line + 1], start, cols).is_none() {
            return false;
        }
        line += 1;
    }
    false
}

/// Whether the row's first word is a list marker (`-`, `*`, `12.`) or a
/// shell prompt (`user@host:~$`, or `user@host:~$pwd` with the command typed
/// right after it) rather than continuing a URL.
///
/// A URL row that fills its width can also end in `%` or `#` before the wrap
/// padding, so a prompt terminator only counts short of the wrap edge.
fn starts_list_item_or_prompt(row: &Row<Cell>, word: Range<usize>, cols: usize) -> bool {
    let followed_by_blank = word.end < cols && is_blank(&row[Column(word.end)]);
    (followed_by_blank
        && (is_marker(row_chars(row, word.clone()))
            || (ends_before_wrap_edge(&word, cols) && PROMPT_TERMINATORS.contains(&row[Column(word.end - 1)].c))))
        || is_prompt_with_command(row_chars(row, word))
}

/// Whether a word has the shape `user@host:path` followed by a prompt
/// terminator and a command typed without a space, such as `user@host:~$pwd`.
fn is_prompt_with_command(chars: impl Iterator<Item = char>) -> bool {
    #[derive(Clone, Copy)]
    enum Part {
        User,
        Host,
        Path,
        Command,
    }
    let mut part = Part::User;
    let mut part_len = 0;
    for character in chars {
        let name_char = character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-');
        part = match part {
            Part::User if character == '@' && part_len > 0 => Part::Host,
            Part::Host if character == ':' && part_len > 0 => Part::Path,
            Part::User | Part::Host if name_char => {
                part_len += 1;
                continue;
            }
            Part::Path if PROMPT_TERMINATORS.contains(&character) => Part::Command,
            Part::Path => Part::Path,
            Part::Command => return true,
            Part::User | Part::Host => return false,
        };
        part_len = 0;
    }
    false
}

/// Whether `word` ends a sentence rather than a wrapped URL chunk: it stops
/// short of the wrap edge and ends in sentence punctuation, or in a question
/// mark or a closing delimiter with no other URL syntax before it. Line
/// breakers also split URLs after `?`, but such chunks carry other delimiters
/// or joiners, and a closer that `segment_opens` reports open balances the URL.
fn word_ends_sentence(
    row: &Row<Cell>,
    word: &Range<usize>,
    cols: usize,
    segment_opens: impl FnOnce() -> [bool; 3],
) -> bool {
    if !ends_before_wrap_edge(word, cols) {
        return false;
    }
    let bare = || {
        !row_chars(row, word.start..word.end - 1)
            .any(|character| URL_DELIMITERS.contains(&character) || URL_WORD_JOINERS.contains(&character))
    };
    let last = row[Column(word.end - 1)].c;
    if let Some(pair) = DELIMITER_PAIRS.iter().position(|(_, close)| *close == last) {
        return bare() && !segment_opens()[pair];
    }
    if last == '?' {
        return bare();
    }
    SENTENCE_PUNCTUATION.contains(&last)
}

/// For each of [`DELIMITER_PAIRS`], whether `chars` leave an opener unmatched.
fn unmatched_openers(chars: impl Iterator<Item = char>) -> [bool; 3] {
    let mut depth = [0usize; 3];
    for character in chars {
        for (pair, (open, close)) in DELIMITER_PAIRS.iter().enumerate() {
            if character == *open {
                depth[pair] += 1;
            } else if character == *close {
                depth[pair] = depth[pair].saturating_sub(1);
            }
        }
    }
    depth.map(|open| open > 0)
}

/// Whether `word` stops short of the wrap edge. A word that runs into the
/// edge is a wrapped URL chunk, whatever punctuation it happens to end with.
fn ends_before_wrap_edge(word: &Range<usize>, cols: usize) -> bool {
    cols - word.end > MAX_WRAP_PADDING
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

/// The URL segment starting at `segment_start` on row `line`, preceded by the
/// full-width rows of URL text that wrap into it, up to
/// [`MAX_URL_CONTEXT_ROWS`] rows in all. The walk stops at the segment that
/// starts the URL, so the text starts with the URL's own scheme when the URL
/// begins within that reach, and carries any delimiters a continuation might
/// close.
fn segment_text_with_rows_above(grid: &Grid<Cell>, cols: usize, mut line: Line, mut segment_start: usize) -> String {
    let mut text = String::new();
    for _ in 0..MAX_URL_CONTEXT_ROWS {
        let row = &grid[line];
        let Some(end) = last_content_column(row, cols) else {
            break;
        };
        let segment = row_chars(row, segment_start..end + 1);
        let starts_url = starts_with_scheme(segment.clone());
        text.insert_str(0, &segment.collect::<String>());
        // A segment that starts its own URL is where that URL begins, just as
        // a continuation that starts a new URL never joins the row above.
        if starts_url || first_content_column(row, cols) != Some(segment_start) || line <= grid.topmost_line() {
            break;
        }
        let above = &grid[line - 1];
        let Some(above_start) = last_content_column(above, cols)
            .filter(|&above_end| cols - 1 - above_end <= MAX_WRAP_PADDING)
            .and_then(|above_end| url_segment_start(above, above_end))
        else {
            break;
        };
        line -= 1;
        segment_start = above_start;
    }
    text
}

/// Whether text starts with a URI scheme and `://`, such as `https://` or
/// `ssh://`: the start of a new URL, even one Horizon does not open.
fn starts_with_scheme(mut chars: impl Iterator<Item = char>) -> bool {
    if !chars.next().is_some_and(|first| first.is_ascii_alphabetic()) {
        return false;
    }
    for character in chars.by_ref() {
        if character == ':' {
            return chars.next() == Some('/') && chars.next() == Some('/');
        }
        if !(character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')) {
            return false;
        }
    }
    false
}

/// Whether text starts like an absolute or home-relative file path.
fn starts_path(mut chars: impl Iterator<Item = char>) -> bool {
    match chars.next() {
        Some('/') => true,
        Some('~') => chars.next() == Some('/'),
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
mod tests;
