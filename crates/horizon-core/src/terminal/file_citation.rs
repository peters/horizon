use std::ops::Range;

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
    let path = path?;
    if path.trim().is_empty() || !path.is_ascii() || path.chars().any(char::is_control) {
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

#[must_use]
pub fn next_terminal_file_citation(
    text: &str,
    mut search_start: usize,
) -> Option<(Range<usize>, TerminalFileCitation)> {
    while let Some(relative_start) = text.get(search_start..)?.find(TERMINAL_FILE_CITATION_PREFIX) {
        let start = search_start + relative_start;
        let Some(end) = terminal_file_citation_end(text, start) else {
            search_start = start + TERMINAL_FILE_CITATION_PREFIX.len();
            continue;
        };
        if let Some(citation) = parse_terminal_file_citation(&text[start..end]) {
            return Some((start..end, citation));
        }
        search_start = start + TERMINAL_FILE_CITATION_PREFIX.len();
    }
    None
}

#[must_use]
fn terminal_file_citation_end(text: &str, start: usize) -> Option<usize> {
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
        if remainder
            .chars()
            .next()
            .is_some_and(|character| !character.is_whitespace())
        {
            return None;
        }
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
    use super::{next_terminal_file_citation, parse_terminal_file_citation};
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
    fn finds_one_of_multiple_citations() {
        let text = concat!(
            "first :codex-file-citation{path=\"/tmp/a.pdf\" purpose=\"source\"} ",
            "second :codex-file-citation{path=\"/tmp/b.pdf\" purpose=\"source\"}",
        );
        let (_, first) = next_terminal_file_citation(text, 0).expect("first citation");
        let second_start = text.find("second").expect("second marker");
        let (_, second) = next_terminal_file_citation(text, second_start).expect("second citation");
        assert_eq!(first.path, "/tmp/a.pdf");
        assert_eq!(second.path, "/tmp/b.pdf");
    }
    #[test]
    fn preserves_exact_ascii_paths_and_rejects_unicode_paths() {
        let exact = r#":codex-file-citation{path=" /tmp/report.pdf " purpose="output"}"#;
        let unicode = concat!(
            r#":codex-file-citation{path="/tmp/resume"#,
            "\u{301}",
            r#".pdf" purpose="source"}"#,
        );
        let concatenated = r#":codex-file-citation{path="/tmp/a"purpose="source"}"#;
        assert_eq!(
            parse_terminal_file_citation(exact).map(|citation| citation.path),
            Some(" /tmp/report.pdf ".into())
        );
        assert_eq!(parse_terminal_file_citation(unicode), None);
        assert_eq!(parse_terminal_file_citation(concatenated), None);
    }
    #[test]
    fn recovers_a_valid_citation_after_a_malformed_candidate() {
        let text = concat!(
            ":codex-file-citation{broken ",
            ":codex-file-citation{path=\"/tmp/report.pdf\" purpose=\"output\"}",
        );
        let (_, citation) = next_terminal_file_citation(text, 0).expect("valid citation");
        assert_eq!(citation.path, "/tmp/report.pdf");
    }
}
