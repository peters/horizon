use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::{Term, viewport_to_point};

pub const TERMINAL_FILE_CITATION_PREFIX: &str = ":codex-file-citation{";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFileCitation {
    pub path: String,
    pub display_label: String,
}

#[must_use]
pub fn parse_terminal_file_citation(token: &str) -> Option<TerminalFileCitation> {
    let body = token.strip_prefix(TERMINAL_FILE_CITATION_PREFIX)?.strip_suffix('}')?;
    let (path, purpose) = parse_attributes(body)?;
    let path = path?.trim().to_owned();
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    let kind = match purpose?.as_str() {
        "source" => "Source",
        "output" => "File",
        _ => return None,
    };
    let file_name = path
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path.as_str())
        .to_owned();

    Some(TerminalFileCitation {
        path,
        display_label: format!("{kind} · {file_name}"),
    })
}

pub(super) fn file_citation_target_at_column(chars: &[char], column: usize) -> Option<String> {
    let text: String = chars.iter().collect();
    let mut search_start = 0;
    while let Some(relative_start) = text[search_start..].find(TERMINAL_FILE_CITATION_PREFIX) {
        let start = search_start + relative_start;
        let end = terminal_file_citation_end(&text, start)?;
        let start_column = text[..start].chars().count();
        let end_column = start_column + text[start..end].chars().count();
        if (start_column..end_column).contains(&column) {
            return parse_terminal_file_citation(&text[start..end]).map(|citation| citation.path);
        }
        search_start = end;
    }
    None
}

#[must_use]
pub fn terminal_file_citation_end(text: &str, start: usize) -> Option<usize> {
    let body_start = start + TERMINAL_FILE_CITATION_PREFIX.len();
    let (mut quoted, mut escaped) = (false, false);
    for (offset, character) in text.get(body_start..)?.char_indices() {
        if escaped {
            escaped = false;
        } else {
            match character {
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '}' if !quoted => return Some(body_start + offset + 1),
                _ => {}
            }
        }
    }
    None
}

pub(super) fn wrapped_line_contains_file_citation<T>(term: &Term<T>, cols: usize, row: usize, col: usize) -> bool {
    if col >= cols {
        return false;
    }
    let grid = term.grid();
    if row >= grid.screen_lines() {
        return false;
    }
    let point = viewport_to_point(grid.display_offset(), Point::new(row, Column(col)));
    let start = term.line_search_left(point);
    let end = term.line_search_right(point);
    let prefix = TERMINAL_FILE_CITATION_PREFIX.as_bytes();
    let mut matched = 0;
    let mut line = start.line;

    loop {
        for column in 0..cols {
            let character = grid[line][Column(column)].c;
            if character == char::from(prefix[matched]) {
                matched += 1;
                if matched == prefix.len() {
                    return true;
                }
            } else {
                matched = usize::from(character == char::from(prefix[0]));
            }
        }
        if line == end.line {
            return false;
        }
        line += 1;
    }
}

fn parse_attributes(body: &str) -> Option<(Option<String>, Option<String>)> {
    let (mut path, mut purpose) = (None, None);
    let mut rest = body;
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let key_end = rest
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .unwrap_or(rest.len());
        if key_end == 0 {
            return None;
        }
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start();
        rest = rest.strip_prefix('=')?.trim_start();
        let (value, remainder) = parse_quoted_value(rest)?;
        rest = remainder;
        match key {
            "path" => path = Some(value),
            "purpose" => purpose = Some(value),
            _ => {}
        }
    }
    Some((path, purpose))
}

fn parse_quoted_value(input: &str) -> Option<(String, &str)> {
    let mut rest = input.strip_prefix('"')?;
    let mut value = String::new();
    while let Some(character) = rest.chars().next() {
        rest = &rest[character.len_utf8()..];
        match character {
            '"' => return Some((value, rest)),
            '\\' => {
                let escaped = rest.chars().next()?;
                if matches!(escaped, '"' | '\\') {
                    value.push(escaped);
                    rest = &rest[escaped.len_utf8()..];
                } else {
                    value.push('\\');
                }
            }
            _ => value.push(character),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{file_citation_target_at_column, parse_terminal_file_citation};

    #[test]
    fn parses_source_citation_and_uses_the_file_name() {
        let citation = parse_terminal_file_citation(
            r#":codex-file-citation{path="/tmp/guides/Linux install.pdf" purpose="source"}"#,
        )
        .expect("citation");

        assert_eq!(citation.path, "/tmp/guides/Linux install.pdf");
        assert_eq!(citation.display_label, "Source · Linux install.pdf");
    }

    #[test]
    fn finds_click_target_inside_one_of_multiple_citations() {
        let text = concat!(
            "first :codex-file-citation{path=\"/tmp/a.pdf\" purpose=\"source\"} ",
            "second :codex-file-citation{path=\"/tmp/b.pdf\" purpose=\"source\"}",
        );
        let chars: Vec<char> = text.chars().collect();
        let column = text.find("b.pdf").expect("second path");

        assert_eq!(
            file_citation_target_at_column(&chars, column),
            Some("/tmp/b.pdf".to_owned())
        );
    }
}
