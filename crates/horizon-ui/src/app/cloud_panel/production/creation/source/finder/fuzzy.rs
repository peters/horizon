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

/// The best-scoring way to find `word` in order in `text`: over every place each letter
/// occurs, not only the first that fits, so a later run or start of a word wins over
/// scattered letters before it. Among equals the earliest places are kept.
fn word_match(word: &[char], text: &[char]) -> Option<Match> {
    if !fits(word, text) {
        return None;
    }
    let name_starts = text.iter().rposition(|&c| c == '/').map_or(0, |slash| slash + 1);
    // What a letter found at `at` is worth by itself.
    let gain = |at: usize| 10 + if boundary(text, at) { 25 } else { 0 } + if at >= name_starts { 5 } else { 0 };
    // What following the letter at `previous` with one at `at` is worth: a run, or a gap.
    let link = |previous: usize, at: usize| {
        if previous + 1 == at {
            15
        } else {
            -i32::try_from((at - previous - 1).min(10)).unwrap_or(10)
        }
    };
    // For each letter of the word, each place it occurs with the best score of the word up
    // to it there, and the place in the layer before that score came from.
    let mut layers: Vec<Vec<(usize, i32, usize)>> = Vec::with_capacity(word.len());
    for (index, wanted) in word.iter().enumerate() {
        let mut layer = Vec::new();
        for at in (0..text.len()).filter(|&at| text[at] == *wanted) {
            let Some(before) = index.checked_sub(1).map(|previous| &layers[previous]) else {
                layer.push((at, gain(at), usize::MAX));
                continue;
            };
            let best = before
                .iter()
                .enumerate()
                .filter(|(_, (previous, _, _))| *previous < at)
                .map(|(from, (previous, score, _))| (score + link(*previous, at), from))
                .max_by_key(|&(score, from)| (score, std::cmp::Reverse(from)));
            if let Some((score, from)) = best {
                layer.push((at, score + gain(at), from));
            }
        }
        if layer.is_empty() {
            return None;
        }
        layers.push(layer);
    }
    let last = layers.last()?;
    let (mut from, mut score) = last
        .iter()
        .enumerate()
        .map(|(from, (_, score, _))| (from, *score))
        .max_by_key(|&(from, score)| (score, std::cmp::Reverse(from)))?;
    let mut positions = vec![0; word.len()];
    for (index, layer) in layers.iter().enumerate().rev() {
        let (at, _, back) = layer[from];
        positions[index] = at;
        from = back;
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
    fn a_later_run_wins_over_scattered_letters_before_it() {
        let found = score("ab", "a-ab").unwrap();
        assert_eq!(found.positions, [2, 3]);
        assert!(found.score > score("ab", "axxb").unwrap().score);
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
