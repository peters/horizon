//! Fuzzy matching for the source field: `hzwb` finds `horizon/web-board`. Every typed
//! character must appear in order; starts of words and runs score higher than scattered
//! letters, so the name meant usually comes first.

/// A match of a query in a text: its score and the character positions it used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Match {
    pub score: i32,
    pub positions: Vec<usize>,
}

/// Matches every word of `query` in `text`, ignoring case. `None` when a word is missing.
pub(super) fn score(query: &str, text: &str) -> Option<Match> {
    // One lowercase letter per letter, so a position lights the letter it came from.
    let text: Vec<char> = text.chars().map(lower).collect();
    let mut total = Match {
        score: 0,
        positions: Vec::new(),
    };
    for word in query.split_whitespace() {
        let word: Vec<char> = word.chars().map(lower).collect();
        let found = word_match(&word, &text)?;
        total.score += found.score;
        total.positions.extend(found.positions);
    }
    total.positions.sort_unstable();
    total.positions.dedup();
    Some(total)
}

fn lower(letter: char) -> char {
    letter.to_lowercase().next().unwrap_or(letter)
}

fn word_match(word: &[char], text: &[char]) -> Option<Match> {
    let mut positions = Vec::with_capacity(word.len());
    let mut from = 0;
    for (index, wanted) in word.iter().enumerate() {
        let rest = &word[index + 1..];
        let candidates = (from..text.len()).filter(|&at| text[at] == *wanted);
        let mut chosen = None;
        for at in candidates {
            // The first place that still leaves room for the rest of the word, unless a
            // later one continues a run or starts a word.
            if !fits(rest, &text[at + 1..]) {
                break;
            }
            let follows = positions.last().is_some_and(|&last| last + 1 == at);
            if chosen.is_none() {
                chosen = Some(at);
            }
            if follows || boundary(text, at) {
                chosen = Some(at);
                break;
            }
        }
        let at = chosen?;
        positions.push(at);
        from = at + 1;
    }
    let mut score = 0;
    let name_starts = text.iter().rposition(|&c| c == '/').map_or(0, |slash| slash + 1);
    for (index, &at) in positions.iter().enumerate() {
        score += 10;
        if boundary(text, at) {
            score += 25;
        }
        if index > 0 && positions[index - 1] + 1 == at {
            score += 15;
        } else if index > 0 {
            let gap = at - positions[index - 1] - 1;
            score -= i32::try_from(gap.min(10)).unwrap_or(10);
        }
        if at >= name_starts {
            score += 5;
        }
    }
    let name: String = text[name_starts..].iter().collect();
    let word: String = word.iter().collect();
    if name == word {
        score += 120;
    } else if name.starts_with(&word) {
        score += 60;
    }
    // Shorter texts with the same letters are the closer match.
    score -= i32::try_from(text.len().min(60)).unwrap_or(60) / 4;
    Some(Match { score, positions })
}

/// Whether `word` still appears in order somewhere in `text`.
fn fits(word: &[char], text: &[char]) -> bool {
    let mut letters = text.iter();
    word.iter().all(|wanted| letters.any(|c| c == wanted))
}

fn boundary(text: &[char], at: usize) -> bool {
    at == 0 || matches!(text[at - 1], '/' | '-' | '_' | '.' | ' ')
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn letters_must_come_in_order() {
        assert!(score("hzn", "horizon").is_some());
        assert!(score("nzh", "horizon").is_none());
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn starts_of_words_are_preferred_and_highlighted() {
        let found = score("wb", "acme/web-board").unwrap();
        assert_eq!(found.positions, [5, 9], "w of web, b of board");
    }

    #[test]
    fn the_name_meant_ranks_first() {
        let rank = |query: &str| {
            let mut names = ["acme/hello-world", "acme/web", "acme/website-old", "acme/awebsome"];
            names.sort_by_key(|name| -score(query, name).map_or(i32::MIN + 1, |found| found.score));
            names[0]
        };
        assert_eq!(rank("web"), "acme/web");
        assert_eq!(rank("hw"), "acme/hello-world");
        assert_eq!(rank("webold"), "acme/website-old");
    }

    #[test]
    fn every_word_must_match_and_case_is_ignored() {
        assert!(score("acme WEB", "Acme/Web").is_some());
        assert!(score("acme mobile", "acme/web").is_none());
    }
}
