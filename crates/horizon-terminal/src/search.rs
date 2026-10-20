//! Literal and regex search in lines of terminal text.

use regex::RegexBuilder;

/// A single match within the searched lines.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    /// Zero-based line index within the extracted text.
    pub line_index: usize,
    /// Byte offset of the match start within the line.
    pub byte_offset: usize,
    /// Length of the match in bytes.
    pub byte_len: usize,
}

/// Options controlling how the search is performed.
#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

/// The most matches that one search returns, so that a search of a deep history stays small.
pub const MAX_MATCHES: usize = 200;

/// The matches of `query` in `lines`, at most [`MAX_MATCHES`], in line order. An empty query or an
/// invalid regex finds nothing.
#[must_use]
pub fn search_lines(lines: &[String], query: &str, options: &SearchOptions) -> Vec<SearchMatch> {
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
    if case_sensitive {
        for (line_index, line) in lines.iter().enumerate() {
            let mut start = 0;
            while let Some(pos) = line[start..].find(query) {
                let byte_offset = start + pos;
                matches.push(SearchMatch {
                    line_index,
                    byte_offset,
                    byte_len: query.len(),
                });
                start = byte_offset + query.len().max(1);
                if matches.len() >= MAX_MATCHES {
                    return;
                }
            }
        }
        return;
    }

    let folded_query = lowercase_with_byte_offsets(query).folded;

    for (line_index, line) in lines.iter().enumerate() {
        let folded_line = lowercase_with_byte_offsets(line);
        let mut start = 0;

        while let Some(pos) = folded_line.folded[start..].find(&folded_query) {
            let folded_start = start + pos;
            let folded_end = folded_start + folded_query.len();
            let byte_offset = folded_line.start_offsets[folded_start];
            let byte_end = folded_line.end_offsets[folded_end - 1];

            matches.push(SearchMatch {
                line_index,
                byte_offset,
                byte_len: byte_end.saturating_sub(byte_offset),
            });
            start = folded_end.max(folded_start + 1);
            if matches.len() >= MAX_MATCHES {
                return;
            }
        }
    }
}

fn search_lines_regex(lines: &[String], pattern: &str, case_sensitive: bool, matches: &mut Vec<SearchMatch>) {
    let Ok(re) = RegexBuilder::new(pattern).case_insensitive(!case_sensitive).build() else {
        return;
    };

    for (line_index, line) in lines.iter().enumerate() {
        for m in re.find_iter(line) {
            matches.push(SearchMatch {
                line_index,
                byte_offset: m.start(),
                byte_len: m.len(),
            });
            if matches.len() >= MAX_MATCHES {
                return;
            }
        }
    }
}

struct LowercasedWithOffsets {
    folded: String,
    start_offsets: Vec<usize>,
    end_offsets: Vec<usize>,
}

fn lowercase_with_byte_offsets(input: &str) -> LowercasedWithOffsets {
    let mut folded = String::new();
    let mut start_offsets = Vec::new();
    let mut end_offsets = Vec::new();

    for (byte_offset, ch) in input.char_indices() {
        let char_end = byte_offset + ch.len_utf8();
        for lowered in ch.to_lowercase() {
            let mut utf8 = [0; 4];
            let lowered = lowered.encode_utf8(&mut utf8);
            start_offsets.extend(std::iter::repeat_n(byte_offset, lowered.len()));
            end_offsets.extend(std::iter::repeat_n(char_end, lowered.len()));
            folded.push_str(lowered);
        }
    }

    LowercasedWithOffsets {
        folded,
        start_offsets,
        end_offsets,
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
    fn literal_case_insensitive_matches_unicode_letters() {
        let text = lines(&["åäö", "ÅÄÖ", "no match"]);
        let results = search_lines(&text, "å", &SearchOptions::default());

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].line_index, 0);
        assert_eq!(results[0].byte_offset, 0);
        assert_eq!(results[1].line_index, 1);
        assert_eq!(results[1].byte_offset, 0);
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
    fn regex_case_insensitive_matches_unicode_letters() {
        let text = lines(&["build Å done", "build å done"]);
        let opts = SearchOptions {
            regex: true,
            ..SearchOptions::default()
        };
        let results = search_lines(&text, "å", &opts);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].byte_offset, "build ".len());
        assert_eq!(results[1].byte_offset, "build ".len());
    }

    #[test]
    fn literal_search_matches_variation_selector_sequences() {
        let text = lines(&["symbols: ✈️ ♥ ©", "symbols: ✈ ♥ ©"]);
        let results = search_lines(&text, "✈️", &SearchOptions::default());

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_index, 0);
        assert_eq!(results[0].byte_offset, "symbols: ".len());
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
