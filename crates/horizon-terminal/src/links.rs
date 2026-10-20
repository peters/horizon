//! URL, file path and OSC 8 hyperlink detection at one cell.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags, Hyperlink};

const URL_SCHEMES: [&str; 3] = ["https://", "http://", "file://"];

/// The OSC 8 hyperlink with a URI at `point`, also on the spacer cell of a wide glyph. It shares
/// the cell's hyperlink without a copy of the URI, so a check on every pointer move costs no
/// allocation.
#[must_use]
pub fn hyperlink_at<T>(term: &Term<T>, point: Point) -> Option<Hyperlink> {
    let grid = term.grid();
    let col = point.column.0;
    if col >= grid.columns() || point.line < grid.topmost_line() || point.line > grid.bottommost_line() {
        return None;
    }
    let cell = &grid[point.line][Column(col)];
    nonempty_hyperlink(cell).or_else(|| {
        (cell.flags.contains(Flags::WIDE_CHAR_SPACER) && col > 0)
            .then(|| nonempty_hyperlink(&grid[point.line][Column(col - 1)]))
            .flatten()
    })
}

fn nonempty_hyperlink(cell: &Cell) -> Option<Hyperlink> {
    let hyperlink = cell.hyperlink()?;
    (!hyperlink.uri().is_empty()).then_some(hyperlink)
}

pub(crate) fn find_url_at_column(chars: &[char], col: usize) -> Option<String> {
    for scheme in URL_SCHEMES {
        let scheme_chars: Vec<char> = scheme.chars().collect();
        let scheme_len = scheme_chars.len();
        if chars.len() < scheme_len {
            continue;
        }
        for start in 0..=chars.len() - scheme_len {
            if chars[start..start + scheme_len] != *scheme_chars {
                continue;
            }
            let end = url_end_column(chars, start);
            if col >= start && col < end {
                return Some(chars[start..end].iter().collect());
            }
        }
    }
    None
}

fn url_end_column(chars: &[char], start: usize) -> usize {
    let mut end = chars.len();
    for (index, character) in chars.iter().enumerate().skip(start) {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'') {
            end = index;
            break;
        }
    }
    strip_trailing_url_chars(chars, start, end)
}

fn strip_trailing_url_chars(chars: &[char], start: usize, mut end: usize) -> usize {
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | '!' | '?') {
        end -= 1;
    }

    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        while end > start && chars[end - 1] == close && unmatched_closing_delimiter(chars, start, end, open, close) {
            end -= 1;
        }
    }

    end
}

fn unmatched_closing_delimiter(chars: &[char], start: usize, end: usize, open: char, close: char) -> bool {
    let mut balance = 0usize;
    for character in &chars[start..end] {
        if *character == open {
            balance += 1;
        } else if *character == close {
            if balance == 0 {
                return true;
            }
            balance -= 1;
        }
    }
    false
}

pub(crate) fn find_file_path_at_column(chars: &[char], col: usize) -> Option<String> {
    let mut index = 0;
    while index < chars.len() {
        let is_path_start = (chars[index] == '/'
            || (chars[index] == '~' && index + 1 < chars.len() && chars[index + 1] == '/'))
            && (index == 0 || is_path_boundary(chars[index - 1]));
        if !is_path_start {
            index += 1;
            continue;
        }
        let start = index;
        let end = path_end_column(chars, start);
        let path = strip_line_col_suffix_chars(&chars[start..end]);
        if path.len() > 1 && col >= start && col < start + path.len() {
            return Some(path.iter().collect());
        }
        index = end;
    }
    None
}

fn is_path_boundary(character: char) -> bool {
    character.is_whitespace() || matches!(character, '"' | '\'' | '(' | '[' | '{' | '<' | '=' | ':')
}

fn path_end_column(chars: &[char], start: usize) -> usize {
    let mut end = chars.len();
    for (index, character) in chars.iter().enumerate().skip(start) {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"' | '\'' | ')' | ']' | '}') {
            end = index;
            break;
        }
    }
    while end > start && matches!(chars[end - 1], '.' | ',' | ';' | '!' | '?') {
        end -= 1;
    }
    end
}

fn strip_line_col_suffix_chars(chars: &[char]) -> &[char] {
    let mut result = chars;
    loop {
        let Some(colon_pos) = result.iter().rposition(|character| *character == ':') else {
            return result;
        };
        let suffix = &result[colon_pos + 1..];
        if !suffix.is_empty() && suffix.iter().all(char::is_ascii_digit) {
            result = &result[..colon_pos];
        } else {
            return result;
        }
    }
}

#[cfg(test)]
mod tests;
