//! Rows next to a URL that stay separate: prose, paths, prompts, list items,
//! status lines and new URLs.

use super::{COLS, term_with_rows, texts, url_at};

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
    for next in [
        "Thanks for reading.",
        "Thanks.",
        "see the docs for more",
        "Continue?",
        "Overwrite? [y/N]",
    ] {
        let term = term_with_rows(COLS, 4, &[first.clone(), next.to_string()]);

        assert_eq!(url_at(&term, COLS, 0, 5), Some(first.clone()), "next row {next:?}");
        assert_eq!(url_at(&term, COLS, 1, 2), None, "next row {next:?}");
    }
}

#[test]
fn path_below_a_row_filling_url_stays_a_path() {
    let first = format!("https://example.com/{}", "a".repeat(COLS - 20));
    for next in ["/tmp/file.rs", "~/work/notes-v2.md"] {
        let term = term_with_rows(COLS, 4, &[first.clone(), next.to_string()]);

        assert_eq!(url_at(&term, COLS, 0, 5), Some(first.clone()), "next row {next:?}");
        assert_eq!(url_at(&term, COLS, 1, 3), None, "next row {next:?}");
    }
}

#[test]
fn row_filling_path_without_a_later_query_stays_a_path() {
    let first = format!("https://example.com/{}", "v".repeat(COLS - 20));
    let path = format!("/usr/share/{}", "d".repeat(COLS - 11));
    let term = term_with_rows(COLS, 4, &[first.clone(), path, "done".to_string()]);

    assert_eq!(url_at(&term, COLS, 0, 3), Some(first));
    assert_eq!(url_at(&term, COLS, 1, 3), None);
}

#[test]
fn shell_prompt_after_a_url_is_not_a_continuation() {
    let term = term_with_rows(COLS, 4, &texts(&["https://example.com/api/", "peters@host:~/work$"]));
    assert_eq!(url_at(&term, COLS, 0, 5).as_deref(), Some("https://example.com/api/"));

    let first = format!("https://a.example/{}", "a".repeat(COLS - 18));
    for prompt in ["root@box:/workspace# ls", "host% ls", "host%"] {
        let term = term_with_rows(COLS, 4, &[first.clone(), prompt.to_string()]);
        assert_eq!(url_at(&term, COLS, 0, 5), Some(first.clone()), "prompt {prompt:?}");
    }
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
