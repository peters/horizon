//! Rows that a program broke through the middle of a URL join into one URL.

use alacritty_terminal::grid::Scroll;

use super::{COLS, OAUTH_URL, RowJoin, hard_wrap, logical_line_at_viewport_point, term_with_rows, texts, url_at};

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
fn url_broken_before_a_path_segment_with_a_query_joins() {
    let first = format!("https://example.com/{}", "a".repeat(COLS - 20));
    let term = term_with_rows(COLS, 4, &[first.clone(), "/authorize?client_id=1".to_string()]);

    assert_eq!(
        url_at(&term, COLS, 1, 3),
        Some(format!("{first}/authorize?client_id=1"))
    );
}

#[test]
fn row_filling_path_segment_joins_when_later_rows_reach_the_query() {
    let first = format!("https://example.com/{}", "v".repeat(COLS - 20));
    let path = format!("/oauth/authorize/{}", "p".repeat(COLS - 17));
    let query = "?client_id=0123456789";
    let term = term_with_rows(COLS, 4, &[first.clone(), path.clone(), query.to_string()]);
    let url = format!("{first}{path}{query}");

    for row in 0..3 {
        assert_eq!(url_at(&term, COLS, row, 3).as_deref(), Some(url.as_str()), "row {row}");
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
fn padded_url_row_ending_in_a_percent_escape_still_joins() {
    let url = format!(
        "https://example.com/{}{}%3Aprofile+user",
        "a".repeat(COLS - 23),
        "b".repeat(COLS - 4)
    );
    let rows: Vec<String> = hard_wrap(&url, "  ", "  ", COLS - 3, COLS - 3)
        .into_iter()
        .map(|row| format!("{row:<width$}", width = COLS - 1))
        .collect();
    assert!(rows[1].trim_end().ends_with('%'), "middle row {:?}", rows[1]);
    let term = term_with_rows(COLS, 4, &rows);

    assert_eq!(url_at(&term, COLS, 0, 5).as_deref(), Some(url.as_str()));
    assert_eq!(url_at(&term, COLS, 2, 3), Some(url));
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

#[test]
fn full_width_row_ending_in_punctuation_keeps_joining() {
    let cols = 20;
    let term = term_with_rows(
        cols,
        4,
        &texts(&["https://example.com/", "aaaaaaaaaaaaaaaaaaa.", "tail"]),
    );

    for row in 0..3 {
        assert_eq!(
            url_at(&term, cols, row, 2).as_deref(),
            Some("https://example.com/aaaaaaaaaaaaaaaaaaa.tail"),
            "row {row}"
        );
    }
}

#[test]
fn file_url_continues_onto_path_shaped_rows() {
    let first = format!("file:///very/long/{}", "x".repeat(COLS - 18));
    let middle = format!("/next-segment/{}", "y".repeat(COLS - 14));
    let term = term_with_rows(COLS, 4, &[first.clone(), middle.clone(), "/final.txt".to_string()]);
    let url = format!("{first}{middle}/final.txt");

    for row in 0..3 {
        assert_eq!(url_at(&term, COLS, row, 3).as_deref(), Some(url.as_str()), "row {row}");
    }
}

#[test]
fn ragged_middle_row_ending_in_a_question_mark_joins() {
    let cols = 60;
    let term = term_with_rows(
        cols,
        4,
        &texts(&[
            "     https://docs.example.com/guides/",
            "     terminal-rendering/long-links?",
            "     topic=terminal&ref=test-0001",
        ]),
    );
    let url = "https://docs.example.com/guides/terminal-rendering/long-links?topic=terminal&ref=test-0001";

    for row in 0..3 {
        assert_eq!(url_at(&term, cols, row, 8).as_deref(), Some(url), "row {row}");
    }
}
