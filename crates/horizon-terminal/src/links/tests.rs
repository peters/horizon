use alacritty_terminal::event::VoidListener;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::vte::ansi;

use super::{find_file_path_at_column, find_url_at_column, hyperlink_at};
use crate::logical_line::{RowJoin, logical_line_at};
use crate::test_size::TestSize;
use crate::{Target, target_at};

fn test_term(rows: usize, cols: usize, bytes: &[u8]) -> Term<VoidListener> {
    let config = term::Config {
        scrolling_history: 256,
        ..term::Config::default()
    };
    let mut term = Term::new(config, &TestSize::new(rows, cols), VoidListener);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(&mut term, bytes);
    term
}

fn at(line: i32, col: usize) -> Point {
    Point::new(Line(line), Column(col))
}

fn uri_at(term: &Term<VoidListener>, point: Point) -> Option<String> {
    hyperlink_at(term, point).map(|hyperlink| hyperlink.uri().to_owned())
}

fn text(target: Option<Target>) -> Option<String> {
    target.map(Target::into_text)
}

#[test]
fn target_at_returns_osc8_uri_for_labeled_text() {
    let term = test_term(24, 80, b"Read \x1b]8;;https://x.ai/terms\x07Terms\x1b]8;;\x07");

    assert_eq!(
        target_at(&term, at(0, 5)),
        Some(Target::Hyperlink("https://x.ai/terms".to_string()))
    );
    assert_eq!(uri_at(&term, at(0, 9)), Some("https://x.ai/terms".to_string()));
    assert_eq!(uri_at(&term, at(0, 0)), None);
    assert_eq!(target_at(&term, at(0, 0)), None);
}

#[test]
fn target_at_joins_hard_wrapped_url_rows_but_not_paths() {
    let url = format!("https://example.com/{}", "a".repeat(100));
    let (url_head, url_tail) = url.split_at(80);
    let path = format!("/tmp/{}", "b".repeat(75));
    let row_filling_url = format!("https://example.com/{}", "c".repeat(60));
    let bytes = format!("{url_head}\r\n{url_tail}\r\n{path}\r\nc/file.rs\r\n{row_filling_url}\r\n/tmp/file.rs");
    let term = test_term(24, 80, bytes.as_bytes());

    assert_eq!(target_at(&term, at(0, 79)), Some(Target::Url(url.clone())));
    assert_eq!(text(target_at(&term, at(1, 3))), Some(url));
    assert_eq!(target_at(&term, at(2, 10)), Some(Target::Path(path)));
    assert_eq!(target_at(&term, at(3, 3)), None);
    assert_eq!(text(target_at(&term, at(4, 30))), Some(row_filling_url));
    assert_eq!(text(target_at(&term, at(5, 3))), Some("/tmp/file.rs".to_string()));
}

#[test]
fn target_at_reads_a_line_in_the_history() {
    let term = test_term(2, 40, b"see https://example.com/history\r\n\r\n\r\nnow");

    assert_eq!(
        target_at(&term, at(-2, 8)),
        Some(Target::Url("https://example.com/history".to_string()))
    );
    assert_eq!(target_at(&term, at(-3, 0)), None);
    assert_eq!(target_at(&term, at(2, 0)), None);
    assert_eq!(target_at(&term, at(0, 40)), None);
}

#[test]
fn wrapped_url_detection_includes_continuation_rows() {
    let url = "https://example.com/very/long/path";
    let term = test_term(4, 12, url.as_bytes());

    let line = logical_line_at(&term, at(2, 4), RowJoin::SoftWraps).expect("wrapped line should be present");

    assert_eq!(find_url_at_column(&line.chars, line.column), Some(url.to_string()));
}

#[test]
fn osc8_hyperlink_is_detected_at_labeled_cells() {
    let term = test_term(4, 40, b"Read \x1b]8;;https://x.ai/terms\x07Terms\x1b]8;;\x07 and more");

    assert_eq!(uri_at(&term, at(0, 5)), Some("https://x.ai/terms".to_string()));
    assert_eq!(uri_at(&term, at(0, 9)), Some("https://x.ai/terms".to_string()));
    assert_eq!(uri_at(&term, at(0, 0)), None);
    assert_eq!(uri_at(&term, at(0, 11)), None);
}

#[test]
fn osc8_hyperlink_takes_priority_over_visible_url_text() {
    let term = test_term(4, 40, b"\x1b]8;;https://x.ai/terms\x07https://example.com\x1b]8;;\x07");

    assert_eq!(uri_at(&term, at(0, 0)), Some("https://x.ai/terms".to_string()));
    assert_eq!(uri_at(&term, at(0, 8)), Some("https://x.ai/terms".to_string()));
    assert_eq!(
        target_at(&term, at(0, 8)),
        Some(Target::Hyperlink("https://x.ai/terms".to_string()))
    );
}

#[test]
fn url_detected_at_clicked_column() {
    let line: Vec<char> = "Created: https://github.com/foo/bar/issues/123".chars().collect();
    assert_eq!(
        find_url_at_column(&line, 10),
        Some("https://github.com/foo/bar/issues/123".to_string()),
    );
}

#[test]
fn click_outside_url_returns_none() {
    let line: Vec<char> = "Created: https://github.com/foo".chars().collect();
    assert_eq!(find_url_at_column(&line, 0), None);
}

#[test]
fn trailing_punctuation_stripped_from_url() {
    let line: Vec<char> = "See https://example.com.".chars().collect();
    assert_eq!(find_url_at_column(&line, 5), Some("https://example.com".to_string()),);
}

#[test]
fn http_and_file_schemes_detected() {
    let line: Vec<char> = "open http://localhost:3000/api".chars().collect();
    assert_eq!(
        find_url_at_column(&line, 6),
        Some("http://localhost:3000/api".to_string()),
    );

    let line: Vec<char> = "file:///home/user/doc.pdf rest".chars().collect();
    assert_eq!(
        find_url_at_column(&line, 0),
        Some("file:///home/user/doc.pdf".to_string()),
    );
}

#[test]
fn markdown_link_target_strips_unmatched_closing_parenthesis() {
    let line: Vec<char> = "[YouPark.no](https://youpark.no)".chars().collect();
    assert_eq!(find_url_at_column(&line, 15), Some("https://youpark.no".to_string()),);
}

#[test]
fn balanced_parentheses_preserved_in_url() {
    let line: Vec<char> = "See https://example.com/a(b)".chars().collect();
    assert_eq!(
        find_url_at_column(&line, 10),
        Some("https://example.com/a(b)".to_string()),
    );
}

#[test]
fn absolute_file_path_detected() {
    let line: Vec<char> = "error at /home/user/project/src/main.rs rest".chars().collect();
    assert_eq!(
        find_file_path_at_column(&line, 10),
        Some("/home/user/project/src/main.rs".to_string()),
    );
}

#[test]
fn home_relative_path_detected() {
    let line: Vec<char> = "see ~/project/config.yaml for details".chars().collect();
    assert_eq!(
        find_file_path_at_column(&line, 5),
        Some("~/project/config.yaml".to_string()),
    );
}

#[test]
fn line_col_suffix_stripped_from_path() {
    let line: Vec<char> = "error: /src/lib.rs:42:15 something".chars().collect();
    assert_eq!(find_file_path_at_column(&line, 8), Some("/src/lib.rs".to_string()),);
}

#[test]
fn click_outside_path_returns_none() {
    let line: Vec<char> = "error at /home/user/file.rs".chars().collect();
    assert_eq!(find_file_path_at_column(&line, 0), None);
}

#[test]
fn bare_slash_not_detected_as_path() {
    let line: Vec<char> = "a / b".chars().collect();
    assert_eq!(find_file_path_at_column(&line, 2), None);
}

#[test]
fn url_takes_priority_over_path() {
    let line: Vec<char> = "see file:///home/user/doc.pdf".chars().collect();
    // find_url_at_column should match file:// scheme first
    assert!(find_url_at_column(&line, 5).is_some());
}
