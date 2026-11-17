//! The board drawn headless: one card pass of a board, and the size that a card
//! of a board opens at.
//!
//! Every number here comes from the approved board mock-up: equal columns of
//! waves, a head row of 44 points, one chip height for every item of the wide
//! grid, one right edge for the values of a column, and the row order and the
//! fold of the narrow list. The grid, the list and the strips beside the board
//! keep their tests in the modules of this one.

mod grid;
mod list;
mod strips;

use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, CornerRadius, Pos2, Rect};
use horizon_browser_control::block::{
    Badge, BlockDocument, BoardColumn, BoardItem, BoardRow, BoardState, Button, Component, LogEntry, Tone, Width,
};
use horizon_core::CardRuntime;

use super::{CardFrames, Owner};
use crate::block_widget::metrics::{BODY_PAD_SIDE, RADIUS_CONTROL, RADIUS_FRAME, WIDE_FORM_MIN_WIDTH};
use crate::block_widget::{CARD_HEIGHT_LIMITS, CardForm, card_height_limits, default_card_size, fitted_card_height};
use crate::theme::ResolvedTheme;

/// A card of `width` points that draws `document`, as the shapes of one pass.
pub(super) fn pass(width: f32, document: &BlockDocument) -> Vec<ClippedShape> {
    let (shapes, _) = CardFrames::new().pass_in(width, document, &mut CardRuntime::default(), Owner::Listening);
    shapes
}

/// The body width of a card `width` points wide.
pub(super) fn body(width: f32) -> f32 {
    width - 2.0 * f32::from(BODY_PAD_SIDE)
}

fn column(label: &str, tag: &str, value: &str, current: bool) -> BoardColumn {
    BoardColumn {
        label: label.to_owned(),
        tag: Some(tag.to_owned()),
        note: None,
        value: Some(value.to_owned()),
        current,
    }
}

fn worker(index: usize) -> BoardRow {
    BoardRow {
        label: format!("helper-{index} · build-{index}"),
        subtitle: Some("16 vCPU".to_owned()),
    }
}

fn item(column: usize, row: usize, title: &str, state: BoardState, value: &str, note: &str) -> BoardItem {
    BoardItem {
        column,
        row,
        title: title.to_owned(),
        state,
        value: Some(value.to_owned()),
        note: Some(note.to_owned()),
        tag: None,
    }
}

/// The board of the approved mock-up: three waves on two workers.
pub(super) fn board() -> Component {
    Component::Board {
        columns: vec![
            column("Understand", "1", "4 m", false),
            column("Change", "2", "10 m", true),
            column("Verify", "3", "20 m", false),
        ],
        rows: vec![worker(1), worker(2)],
        items: vec![
            item(0, 0, "Read the issue", BoardState::Done, "4 m", "1 failing test found"),
            item(1, 0, "Change the redirect", BoardState::Now, "8 m", "edits routes"),
            item(
                1,
                1,
                "Update the notes",
                BoardState::Waiting,
                "~5 m",
                "check: docs build",
            ),
            item(2, 1, "Run the full check", BoardState::Waiting, "~20 m", "cargo test"),
        ],
    }
}

fn board_document() -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![board()],
    }
}

/// The right edge of every filled rectangle with the corner radius of an item:
/// the chips that the board painted.
pub(super) fn chips(shapes: &[ClippedShape]) -> Vec<Rect> {
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect)
                if rect.corner_radius == CornerRadius::same(RADIUS_CONTROL) && rect.stroke.width > 0.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect()
}

/// Where the pass painted `text`.
pub(super) fn painted(shapes: &[ClippedShape], text: &str) -> Option<Rect> {
    shapes.iter().find_map(|clipped| match &clipped.shape {
        Shape::Text(shape) if shape.galley.text() == text => Some(Rect::from_min_size(shape.pos, shape.galley.size())),
        _ => None,
    })
}

/// Whether the pass painted a text that holds `text`.
pub(super) fn painted_holding(shapes: &[ClippedShape], text: &str) -> bool {
    fn inner(shape: &Shape, text: &str) -> bool {
        match shape {
            Shape::Text(shape) => shape.galley.text().contains(text),
            Shape::Vec(shapes) => shapes.iter().any(|shape| inner(shape, text)),
            _ => false,
        }
    }
    shapes.iter().any(|clipped| inner(&clipped.shape, text))
}

fn painted_any(shapes: &[ClippedShape], text: &str) -> bool {
    fn inner(shape: &Shape, text: &str) -> bool {
        match shape {
            Shape::Text(shape) => shape.galley.text() == text,
            Shape::Vec(shapes) => shapes.iter().any(|shape| inner(shape, text)),
            _ => false,
        }
    }
    shapes.iter().any(|clipped| inner(&clipped.shape, text))
}

/// The text that holds `holding`: where the pass painted it, the lines that it
/// took, and the last line as the person reads it.
pub(super) fn painted_text(shapes: &[ClippedShape], holding: &str) -> (Rect, usize, String) {
    shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text().contains(holding) => Some((
                Rect::from_min_size(shape.pos, shape.galley.size()),
                shape.galley.rows.len(),
                shape
                    .galley
                    .rows
                    .last()
                    .map(|placed| placed.row.text())
                    .unwrap_or_default(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the pass painted the text that holds {holding}"))
}

/// A board of three waves on one worker, with `note` as the evidence of the one
/// item of the second wave.
pub(super) fn one_long_note(note: &str) -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![Component::Board {
            columns: vec![
                column("Understand", "1", "4 m", false),
                column("Change", "2", "10 m", true),
                column("Verify", "3", "20 m", false),
            ],
            rows: vec![worker(1)],
            items: vec![item(0, 0, "Push the branch", BoardState::Now, "8 m", note)],
        }],
    }
}

/// The narrow form of [`one_long_note`]: one wave and one item.
pub(super) fn one_long_note_narrow(note: &str) -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Board {
            columns: vec![column("Change", "2", "10 m", true)],
            rows: vec![worker(1)],
            items: vec![item(0, 0, "Push the branch", BoardState::Now, "8 m", note)],
        }],
    }
}

/// A board of `columns` waves, one item in each of them, on one worker.
pub(super) fn board_of(columns: usize) -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![Component::Board {
            columns: (0..columns)
                .map(|index| {
                    column(
                        &format!("Wave {}", index + 1),
                        &(index + 1).to_string(),
                        "5 m",
                        index == 0,
                    )
                })
                .collect(),
            rows: vec![worker(1)],
            items: (0..columns)
                .map(|index| {
                    item(
                        index,
                        0,
                        &format!("Work {}", index + 1),
                        BoardState::Waiting,
                        "~5 m",
                        "cargo test",
                    )
                })
                .collect(),
        }],
    }
}

/// A board of two waves with a callout, a log and a button under it.
pub(super) fn board_with_a_callout_a_log_and_a_button() -> BlockDocument {
    let mut document = board_of(2);
    let Component::Board { columns, rows, items } = document.components.remove(0) else {
        panic!("the fixture holds a board");
    };
    document.components = vec![
        Component::Board { columns, rows, items },
        Component::Callout {
            tone: Tone::Warn,
            caption: Some("Needs you".to_owned()),
            text: "Which address must the redirect use?".to_owned(),
            buttons: vec![Button {
                id: "answer.account".to_owned(),
                label: "The account address".to_owned(),
                tone: Tone::Primary,
            }],
        },
        Component::Log {
            entries: (0..6)
                .map(|index| LogEntry {
                    at: format!("07:3{index}"),
                    text: format!("t{index} done on build-2, 9 rows"),
                })
                .collect(),
        },
        Component::Buttons {
            items: vec![Button {
                id: "stop".to_owned(),
                label: "Stop".to_owned(),
                tone: Tone::Neutral,
            }],
        },
    ];
    document
}

/// A document of a heading that a progress bar follows directly.
pub(super) fn heading_and_progress() -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![
            Component::Heading {
                text: "Fix the login redirect".to_owned(),
                subtitle: Some("Issue #1987 · example-org/shop".to_owned()),
                badge: Some(Badge {
                    text: "Running".to_owned(),
                    tone: Tone::Ok,
                }),
            },
            Component::Progress {
                label: "3 of 8 done".to_owned(),
                percent: 38,
                detail: Some("about 30 min left".to_owned()),
            },
        ],
    }
}

/// The stroke colours of every outlined rectangle of a pass.
pub(super) fn outline_colours(shapes: &[ClippedShape]) -> Vec<Color32> {
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(rect.stroke.color),
            _ => None,
        })
        .collect()
}

/// The size of the glyphs that the pass asked for when it painted `text`.
pub(super) fn painted_font_size(shapes: &[ClippedShape], text: &str) -> f32 {
    shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text() == text => Some(shape.galley.job.sections[0].format.font_id.size),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the card drew {text}"))
}

#[test]
fn a_wide_card_opens_as_wide_as_its_board_needs_and_the_view_allows() {
    let padding = 2.0 * f32::from(BODY_PAD_SIDE);
    let least = |columns: f32| 230.0 + 260.0 * columns + padding;
    let most = |columns: f32| 230.0 + 480.0 * columns + padding;

    for (columns, view, width) in [
        // The half of the view keeps the largest width of a wide card.
        (5, 5120.0, 2400.0),
        // A view of 2560 gives 1280, under the least width of five waves, so
        // the card opens at that least width.
        (5, 2560.0, least(5.0)),
        // A view of 1500 cannot give the least width of five waves.
        (5, 1500.0, 420.0),
        (2, 5120.0, most(2.0)),
        (8, 5120.0, 2400.0),
        (3, 5120.0, most(3.0)),
        (3, 2000.0, least(3.0)),
    ] {
        let document = board_of(columns);
        assert!(
            (default_card_size(&document, view)[0] - width).abs() < 0.01,
            "{columns} waves in a view of {view} open at {width}, not {}",
            default_card_size(&document, view)[0]
        );
    }

    assert_eq!(
        default_card_size(
            &BlockDocument {
                width: Some(Width::Narrow),
                finished: false,
                components: vec![board()],
            },
            5120.0
        )[0],
        420.0,
        "a document that asks for the narrow card keeps the card of today"
    );

    // A wide document that holds no board keeps the rule of round 40.
    let no_board = BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![Component::Divider],
    };
    assert_eq!(default_card_size(&no_board, 5120.0)[0], 2400.0);
    assert_eq!(default_card_size(&no_board, 2000.0)[0], 1100.0);
    assert_eq!(default_card_size(&no_board, 20_000.0)[0], 2400.0);
}

#[test]
fn the_form_of_a_card_changes_at_1000_points_of_its_body() {
    assert_eq!(CardForm::of(WIDE_FORM_MIN_WIDTH - 0.5), CardForm::Narrow);
    assert_eq!(CardForm::of(WIDE_FORM_MIN_WIDTH), CardForm::Wide);
    assert_eq!(body(1100.0), 1068.0);

    // The body of a card that opens at the least wide width draws the wide
    // form, and the same document in a narrow card draws the other one.
    let document = board_document();
    let wide = pass(1100.0, &document);
    let narrow = pass(420.0, &document);
    assert!(
        painted_any(&wide, "Understand") && !painted_holding(&wide, "2 · CHANGE · NOW"),
        "a card of 1100 points draws the grid of the board"
    );
    assert!(
        painted_holding(&narrow, "2 · CHANGE · NOW") && !painted_any(&narrow, "Understand"),
        "a card of 420 points draws the list of the board"
    );
}

#[test]
fn a_wide_card_takes_a_height_up_to_1200_points_and_a_narrow_one_up_to_the_room_of_its_window() {
    assert_eq!(
        card_height_limits(&board_document(), 0.0, 0.0),
        [CARD_HEIGHT_LIMITS[0], 1200.0]
    );
    let narrow = BlockDocument {
        width: None,
        finished: false,
        components: vec![],
    };
    assert_eq!(card_height_limits(&narrow, 0.0, 0.0), CARD_HEIGHT_LIMITS);
    assert_eq!(CARD_HEIGHT_LIMITS[1], 560.0);
    // The room of the view at its zoom lifts the ceiling of a wide card, and the
    // room of the window takes no part in it.
    assert_eq!(card_height_limits(&board_document(), 600.0, 1500.0)[1], 1500.0);
    // The ceiling of a card of the default width follows the room of the window
    // between 560 and 900 points, and the zoom of the view takes no part in it.
    for (window, ceiling) in [(554.0, 560.0), (754.0, 754.0), (1174.0, 900.0), (4000.0, 900.0)] {
        assert_eq!(
            card_height_limits(&narrow, window, 3000.0),
            [CARD_HEIGHT_LIMITS[0], ceiling],
            "a window whose canvas is {window} points high"
        );
    }
    assert_eq!(
        fitted_card_height(
            4000.0,
            false,
            CARD_HEIGHT_LIMITS[0],
            card_height_limits(&board_document(), 0.0, 0.0)
        ),
        1200.0
    );
}

#[test]
fn the_estimate_of_a_board_a_callout_and_a_log_grows_with_their_parts_in_both_forms() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        finished: false,
        components: vec![
            Component::Heading {
                text: "Fix the login redirect".to_owned(),
                subtitle: Some("Issue #1987".to_owned()),
                badge: Some(Badge {
                    text: "Running".to_owned(),
                    tone: Tone::Ok,
                }),
            },
            Component::Progress {
                label: "3 of 8 done".to_owned(),
                percent: 38,
                detail: Some("about 30 min left".to_owned()),
            },
            board(),
            Component::Log {
                entries: vec![LogEntry {
                    at: "07:31".to_owned(),
                    text: "t3 done on build-2, 9 rows".to_owned(),
                }],
            },
            Component::Callout {
                tone: Tone::Warn,
                caption: Some("Needs you".to_owned()),
                text: "Which address must the redirect use?".to_owned(),
                buttons: vec![Button {
                    id: "go".to_owned(),
                    label: "Go".to_owned(),
                    tone: Tone::Primary,
                }],
            },
        ],
    };
    let mut narrow_document = document.clone();
    narrow_document.width = Some(Width::Narrow);
    let narrow = default_card_size(&narrow_document, 0.0);
    let wide = default_card_size(&document, 5120.0);

    for (name, size, width, document) in [
        ("narrow", narrow, 420.0, &narrow_document),
        ("wide", wide, 2400.0, &document),
    ] {
        let measured = CardFrames::new().measured(width, document, &mut CardRuntime::default());
        // The body records the height of its content with the padding of the
        // body, which the estimate of the card also holds. A card stops at the
        // ceiling of its form, and its body scrolls the content above that.
        let estimated = size[1] - crate::block_widget::CARD_CHROME_HEIGHT;
        let measured =
            measured.min(card_height_limits(document, 0.0, 0.0)[1] - crate::block_widget::CARD_CHROME_HEIGHT);
        assert!(
            estimated + 4.0 >= measured,
            "{name}: the estimate holds the content that the body measured: {estimated} against {measured}"
        );
        assert!(
            estimated <= measured + 60.0,
            "{name}: the estimate stays near the measured content: {estimated} against {measured}"
        );
    }

    for (name, component) in [
        ("a board", board()),
        (
            "a callout",
            Component::Callout {
                tone: Tone::Neutral,
                caption: None,
                text: "Which address must the redirect use?".to_owned(),
                buttons: vec![],
            },
        ),
        (
            "a log",
            Component::Log {
                entries: (0..5)
                    .map(|index| LogEntry {
                        at: format!("07:3{index}"),
                        text: "t3 done on build-2, 9 rows".to_owned(),
                    })
                    .collect(),
            },
        ),
    ] {
        let one = BlockDocument {
            width: None,
            finished: false,
            components: vec![component.clone()],
        };
        let two = BlockDocument {
            width: None,
            finished: false,
            components: vec![component.clone(), component],
        };
        let one = default_card_size(&one, 0.0)[1];
        let two = default_card_size(&two, 0.0)[1];
        assert!(
            two > one,
            "{name} of two parts opens a taller card: {two} against {one}"
        );
    }
}

#[test]
fn a_colour_of_the_board_comes_from_the_theme_and_not_from_a_literal() {
    // The board paints the accent of the theme, so the same document follows
    // the light theme without a branch of its own. Each pass keeps its palette
    // on this thread: the theme of the process belongs to every test at once,
    // and the pass of a card is long enough for another test to change it.
    let dark = {
        let _theme = crate::test_egui::with_theme(ResolvedTheme::Dark);
        pass(1400.0, &board_document())
    };
    let light = {
        let _theme = crate::test_egui::with_theme(ResolvedTheme::Light);
        pass(1400.0, &board_document())
    };

    assert_ne!(
        outline_colours(&dark),
        outline_colours(&light),
        "the two themes draw other colours"
    );
}
