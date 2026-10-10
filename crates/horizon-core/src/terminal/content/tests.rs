use std::sync::mpsc;

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::vte::ansi;

use super::{
    RowJoin, append_cell_text, bottom_row_texts, hyperlink_uri_at_viewport_point, logical_line_at_viewport_point,
    screen_row_texts,
};
use crate::terminal::{TerminalDimensions, TerminalEventProxy, TerminalSshTrust, find_url_at_column};

fn reconstruct_line(cells: &[(usize, Cell)]) -> String {
    let mut line = String::new();
    let mut occupied_columns = 0;

    for (column, cell) in cells {
        append_cell_text(&mut line, &mut occupied_columns, *column, cell);
    }

    line
}

#[test]
fn multibyte_glyphs_preserve_following_padding() {
    let accent_cell = Cell {
        c: 'é',
        ..Cell::default()
    };
    let x_cell = Cell {
        c: 'x',
        ..Cell::default()
    };

    let line = reconstruct_line(&[(0, accent_cell), (2, x_cell)]);

    assert_eq!(line, "é x");
}

#[test]
fn combining_marks_stay_attached_to_base_cell() {
    let mut base_cell = Cell {
        c: 'e',
        ..Cell::default()
    };
    base_cell.push_zerowidth('\u{0301}');
    let x_cell = Cell {
        c: 'x',
        ..Cell::default()
    };

    let line = reconstruct_line(&[(0, base_cell), (1, x_cell)]);

    assert_eq!(line, "e\u{0301}x");
}

#[test]
fn variation_selectors_stay_attached_to_base_cell() {
    let mut base_cell = Cell {
        c: '✈',
        ..Cell::default()
    };
    base_cell.push_zerowidth('\u{fe0f}');
    let x_cell = Cell {
        c: 'x',
        ..Cell::default()
    };

    let line = reconstruct_line(&[(0, base_cell), (1, x_cell)]);

    assert_eq!(line, "✈\u{fe0f}x");
}

#[test]
fn wide_glyphs_consume_two_terminal_columns() {
    let wide_cell = Cell {
        c: '你',
        flags: Flags::WIDE_CHAR,
        ..Cell::default()
    };
    let x_cell = Cell {
        c: 'x',
        ..Cell::default()
    };

    let line = reconstruct_line(&[(0, wide_cell), (2, x_cell)]);

    assert_eq!(line, "你x");
}

fn test_term(rows: u16, cols: u16) -> Term<TerminalEventProxy> {
    let (event_tx, _event_rx) = mpsc::channel();
    let dimensions = TerminalDimensions::new(rows, cols);
    let config = term::Config {
        scrolling_history: 256,
        kitty_keyboard: true,
        ..term::Config::default()
    };

    Term::new(
        config,
        &dimensions,
        TerminalEventProxy::new(event_tx, TerminalSshTrust::default()),
    )
}

#[test]
fn bottom_row_texts_only_includes_the_bottom_window() {
    let mut term = test_term(6, 20);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(
        &mut term,
        b"top-one\r\ntop-two\r\nmid-one\r\nmid-two\r\nmid-three\r\nworking-line",
    );

    let lines = bottom_row_texts(&term, 6, 3);

    assert_eq!(lines, vec!["mid-two", "mid-three", "working-line"]);
}

#[test]
fn bottom_row_texts_omits_empty_rows_and_accepts_wide_window() {
    let mut term = test_term(8, 20);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    // Row 0 carries text, rows 1-5 are empty, row 6 has the status line.
    parser.advance(&mut term, b"alpha\r\n\r\n\r\n\r\n\r\n\r\n\xe2\xa0\x8b Working...\n");

    let lines = bottom_row_texts(&term, 8, 4);

    assert_eq!(lines, vec!["\u{280b} Working..."]);

    let all = bottom_row_texts(&term, 8, 16);
    assert_eq!(all, vec!["alpha", "\u{280b} Working..."]);
}

#[test]
fn screen_row_texts_keep_blank_rows_and_occupied_columns() {
    let mut term = test_term(4, 6);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(&mut term, "ab\r\n\r\n\u{4e2d}\u{6587}x".as_bytes());

    let rows = screen_row_texts(&term, 4, 4);

    let expected = [("ab", 2), ("", 0), ("\u{4e2d}\u{6587}x", 5), ("", 0)];
    assert_eq!(rows, expected.map(|(text, columns)| (text.to_owned(), columns)));
}

#[test]
fn screen_row_texts_read_the_live_screen_while_the_view_is_scrolled() {
    let mut term = test_term(3, 10);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(&mut term, b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6");
    term.scroll_display(alacritty_terminal::grid::Scroll::Delta(2));

    let rows: Vec<String> = screen_row_texts(&term, 3, 3)
        .into_iter()
        .map(|(text, _)| text)
        .collect();

    assert_eq!(rows, ["l4", "l5", "l6"]);
}

#[test]
fn screen_row_texts_leave_out_concealed_text() {
    let mut term = test_term(2, 20);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(&mut term, b"key \x1b[8msecret\x1b[28m shown");

    let rows = screen_row_texts(&term, 2, 2);

    assert_eq!(rows[0].0, "key        shown");
}

#[test]
fn wrapped_url_detection_includes_continuation_rows() {
    let url = "https://example.com/very/long/path";
    let mut term = test_term(4, 12);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(&mut term, url.as_bytes());

    let line =
        logical_line_at_viewport_point(&term, 12, 2, 4, RowJoin::SoftWraps).expect("wrapped line should be present");

    assert_eq!(find_url_at_column(&line.chars, line.column), Some(url.to_string()));
}

#[test]
fn osc8_hyperlink_is_detected_at_labeled_cells() {
    let mut term = test_term(4, 40);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(
        &mut term,
        b"Read \x1b]8;;https://x.ai/terms\x07Terms\x1b]8;;\x07 and more",
    );

    assert_eq!(
        hyperlink_uri_at_viewport_point(&term, 40, 0, 5),
        Some("https://x.ai/terms".to_string())
    );
    assert_eq!(
        hyperlink_uri_at_viewport_point(&term, 40, 0, 9),
        Some("https://x.ai/terms".to_string())
    );
    assert_eq!(hyperlink_uri_at_viewport_point(&term, 40, 0, 0), None);
    assert_eq!(hyperlink_uri_at_viewport_point(&term, 40, 0, 11), None);
}

#[test]
fn osc8_hyperlink_takes_priority_over_visible_url_text() {
    let mut term = test_term(4, 40);
    let mut parser = ansi::Processor::<ansi::StdSyncHandler>::default();
    parser.advance(
        &mut term,
        b"\x1b]8;;https://x.ai/terms\x07https://example.com\x1b]8;;\x07",
    );

    assert_eq!(
        hyperlink_uri_at_viewport_point(&term, 40, 0, 0),
        Some("https://x.ai/terms".to_string())
    );
    assert_eq!(
        hyperlink_uri_at_viewport_point(&term, 40, 0, 8),
        Some("https://x.ai/terms".to_string())
    );
}
