use crate::board::Board;
use crate::panel::PanelId;

/// A single match within a terminal panel.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    /// Zero-based line index within the extracted text.
    pub line_index: usize,
    /// Byte offset of the match start within the line.
    pub byte_offset: usize,
    /// Length of the match in bytes.
    pub byte_len: usize,
    /// The full text of the matched line.
    pub line_text: String,
}

/// All matches found in a single panel.
#[derive(Clone, Debug)]
pub struct PanelSearchResult {
    pub panel_id: PanelId,
    pub panel_title: String,
    pub matches: Vec<SearchMatch>,
}

/// Aggregated search results across all panels.
#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    pub panels: Vec<PanelSearchResult>,
    pub total_matches: usize,
}

/// Options controlling how the search is performed.
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

/// Maximum number of scrollback + screen lines to extract per terminal.
/// Keeps search responsive even with deep scrollback buffers.
const MAX_EXTRACT_LINES: usize = 10_000;

/// Maximum number of matches per panel to avoid result explosion.
const MAX_MATCHES_PER_PANEL: usize = 200;

/// Extract visible + scrollback text from a terminal as a vector of lines.
///
/// Each line is trimmed of trailing whitespace. Empty trailing lines are
/// dropped to avoid noise. The extraction locks the terminal mutex once
/// and copies all text in a single pass.
fn extract_terminal_lines(board: &Board, panel_id: PanelId) -> Vec<String> {
    let Some(panel) = board.panel(panel_id) else {
        return Vec::new();
    };
    let Some(terminal) = panel.terminal() else {
        return Vec::new();
    };

    let cols = usize::from(terminal.cols());
    let rows = usize::from(terminal.rows());
    let history = terminal.history_size();
    let total_lines = (history + rows).min(MAX_EXTRACT_LINES);

    terminal.with_renderable_content(|content| {
        let mut lines: Vec<String> = Vec::with_capacity(total_lines);
        let mut current_line = String::with_capacity(cols);
        let mut current_row: Option<i32> = None;

        for indexed in content.display_iter {
            let row = indexed.point.line.0;
            if current_row != Some(row) {
                if let Some(_prev) = current_row {
                    let trimmed = current_line.trim_end().to_string();
                    lines.push(trimmed);
                    current_line.clear();
                }
                current_row = Some(row);
            }
            let col = indexed.point.column.0;
            if indexed.cell.c != ' ' || indexed.cell.zerowidth().is_some() {
                while current_line.len() < col {
                    current_line.push(' ');
                }
                current_line.push(indexed.cell.c);
            }
            if lines.len() >= MAX_EXTRACT_LINES {
                break;
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line.trim_end().to_string());
        }

        // Drop empty trailing lines.
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }

        lines
    })
}

/// Search across all terminal panels on the board.
///
/// Returns structured results grouped by panel, with line context for
/// each match. The search is performed on extracted text snapshots,
/// so it does not hold terminal locks during matching.
#[must_use]
pub fn search_board(board: &Board, query: &str, options: &SearchOptions) -> SearchResults {
    if query.is_empty() {
        return SearchResults::default();
    }

    let panel_ids: Vec<(PanelId, String)> = board
        .panels
        .iter()
        .filter(|p| p.terminal().is_some())
        .map(|p| (p.id, p.display_title().into_owned()))
        .collect();

    let mut results = SearchResults::default();

    for (panel_id, panel_title) in panel_ids {
        let lines = extract_terminal_lines(board, panel_id);
        let matches = search_lines(&lines, query, options);
        if !matches.is_empty() {
            results.total_matches += matches.len();
            results.panels.push(PanelSearchResult {
                panel_id,
                panel_title,
                matches,
            });
        }
    }

    results
}

fn search_lines(lines: &[String], query: &str, options: &SearchOptions) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();

    if options.regex {
        search_lines_regex(lines, query, options.case_sensitive, &mut matches);
    } else {
        search_lines_literal(lines, query, options.case_sensitive, &mut matches);
    }

    matches
}

fn search_lines_literal(lines: &[String], query: &str, case_sensitive: bool, matches: &mut Vec<SearchMatch>) {
    let query_lower = if case_sensitive {
        String::new()
    } else {
        query.to_ascii_lowercase()
    };
    let needle = if case_sensitive { query } else { &query_lower };

    for (line_index, line) in lines.iter().enumerate() {
        let haystack;
        let search_in = if case_sensitive {
            line.as_str()
        } else {
            haystack = line.to_ascii_lowercase();
            haystack.as_str()
        };

        let mut start = 0;
        while let Some(pos) = search_in[start..].find(needle) {
            let byte_offset = start + pos;
            matches.push(SearchMatch {
                line_index,
                byte_offset,
                byte_len: query.len(),
                line_text: line.clone(),
            });
            start = byte_offset + needle.len().max(1);
            if matches.len() >= MAX_MATCHES_PER_PANEL {
                return;
            }
        }
    }
}

fn search_lines_regex(lines: &[String], pattern: &str, case_sensitive: bool, matches: &mut Vec<SearchMatch>) {
    let full_pattern = if case_sensitive {
        pattern.to_string()
    } else {
        format!("(?i){pattern}")
    };

    let Ok(re) = regex_lite::Regex::new(&full_pattern) else {
        return;
    };

    for (line_index, line) in lines.iter().enumerate() {
        for m in re.find_iter(line) {
            matches.push(SearchMatch {
                line_index,
                byte_offset: m.start(),
                byte_len: m.len(),
                line_text: line.clone(),
            });
            if matches.len() >= MAX_MATCHES_PER_PANEL {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchOptions, search_lines};

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn literal_case_insensitive_finds_matches() {
        let text = lines(&["Hello World", "hello again", "no match"]);
        let results = search_lines(&text, "hello", &SearchOptions::default());

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].line_index, 0);
        assert_eq!(results[0].byte_offset, 0);
        assert_eq!(results[1].line_index, 1);
    }

    #[test]
    fn literal_case_sensitive_skips_mismatched_case() {
        let text = lines(&["Hello World", "hello again"]);
        let opts = SearchOptions {
            case_sensitive: true,
            ..SearchOptions::default()
        };
        let results = search_lines(&text, "hello", &opts);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_index, 1);
    }

    #[test]
    fn regex_search_finds_pattern() {
        let text = lines(&["error: file not found", "warning: deprecated", "info: ok"]);
        let opts = SearchOptions {
            regex: true,
            ..SearchOptions::default()
        };
        let results = search_lines(&text, r"error|warning", &opts);

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn empty_query_returns_no_matches() {
        let text = lines(&["some content"]);
        let results = search_lines(&text, "", &SearchOptions::default());

        assert!(results.is_empty());
    }

    #[test]
    fn multiple_matches_on_same_line() {
        let text = lines(&["aa bb aa cc aa"]);
        let results = search_lines(&text, "aa", &SearchOptions::default());

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].byte_offset, 0);
        assert_eq!(results[1].byte_offset, 6);
        assert_eq!(results[2].byte_offset, 12);
    }

    #[test]
    fn invalid_regex_returns_empty() {
        let text = lines(&["test"]);
        let opts = SearchOptions {
            regex: true,
            ..SearchOptions::default()
        };
        let results = search_lines(&text, "[invalid", &opts);

        assert!(results.is_empty());
    }
}
