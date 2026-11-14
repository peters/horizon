//! The board, the callout and the log, drawn headless in both forms.
//!
//! Every number here comes from the approved board mock-up: equal columns of
//! waves, a head row of 44 points, one chip height for every item of the wide
//! grid, one right edge for the values of a column, and the row order and the
//! fold of the narrow list. A press on the fold line is the person's own view of
//! the card, so it changes no click of an agent.

use egui::epaint::{ClippedShape, Shape};
use egui::{Color32, CornerRadius, Pos2, Rect};
use horizon_browser_control::block::{
    Badge, BlockDocument, BoardColumn, BoardItem, BoardRow, BoardState, Button, Component, ListLayout, LogEntry,
    Selection, Tone, Width,
};
use horizon_core::CardRuntime;

use super::{CardFrames, Owner};
use crate::block_widget::metrics::{
    BOARD_HEAD_COLUMN, BOARD_HEAD_HEIGHT, BOARD_ROW_MIN_HEIGHT, BODY_PAD_SIDE, BODY_PAD_TOP, CALLOUT_PAD_Y,
    ITEM_RADIUS, WIDE_FORM_MIN_WIDTH,
};
use crate::block_widget::{CARD_HEIGHT_LIMITS, CardForm, card_height_limits, default_card_size, fitted_card_height};

/// The body width of a card `width` points wide.
fn body(width: f32) -> f32 {
    width - 2.0 * f32::from(BODY_PAD_SIDE)
}

/// A card of `width` points that draws `document`, as the shapes of one pass.
fn pass(width: f32, document: &BlockDocument) -> Vec<ClippedShape> {
    let (shapes, _) = CardFrames::new().pass_in(width, document, &mut CardRuntime::default(), Owner::Listening);
    shapes
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
fn board() -> Component {
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
        components: vec![board()],
    }
}

/// The right edge of every filled rectangle with the corner radius of an item:
/// the chips that the board painted.
fn chips(shapes: &[ClippedShape]) -> Vec<Rect> {
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect) if rect.corner_radius == CornerRadius::same(ITEM_RADIUS) && rect.stroke.width > 0.0 => {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect()
}

/// Where the pass painted `text`.
fn painted(shapes: &[ClippedShape], text: &str) -> Option<Rect> {
    shapes.iter().find_map(|clipped| match &clipped.shape {
        Shape::Text(shape) if shape.galley.text() == text => Some(Rect::from_min_size(shape.pos, shape.galley.size())),
        _ => None,
    })
}

/// Whether the pass painted a text that holds `text`.
fn painted_holding(shapes: &[ClippedShape], text: &str) -> bool {
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

#[test]
fn the_grid_gives_every_wave_the_same_width_after_the_230_points_of_the_workers() {
    let shapes = pass(1400.0, &board_document());
    let understand = painted(&shapes, "Understand").expect("the first wave is drawn");
    let change = painted(&shapes, "Change").expect("the second wave is drawn");
    let verify = painted(&shapes, "Verify").expect("the third wave is drawn");
    let column_width = (body(1400.0) - BOARD_HEAD_COLUMN) / 3.0;
    let first_column = f32::from(BODY_PAD_SIDE) + BOARD_HEAD_COLUMN + 14.0 + 20.0 + 9.0;

    assert!(
        (understand.min.x - first_column).abs() < 1.5,
        "the first wave starts after the column of the workers: {understand:?}"
    );
    assert!(
        (change.min.x - understand.min.x - column_width).abs() < 1.5,
        "two waves are one column apart: {} against {column_width}",
        change.min.x - understand.min.x
    );
    assert!(
        (verify.min.x - change.min.x - column_width).abs() < 1.5,
        "every wave takes the same width: {} against {column_width}",
        verify.min.x - change.min.x
    );
}

#[test]
fn every_item_of_the_wide_grid_takes_one_height_and_a_row_at_least_104_points() {
    let shapes = pass(1400.0, &board_document());
    let chips = chips(&shapes);
    assert_eq!(chips.len(), 4, "every item is a chip");

    let height = chips[0].height();
    assert!(
        chips.iter().all(|chip| (chip.height() - height).abs() < 0.01),
        "one height for every chip: {chips:?}"
    );
    let tops: Vec<f32> = [chips[0].min.y, chips[2].min.y].to_vec();
    let row = Rect::from_min_max(
        Pos2::new(chips[0].min.x, chips[0].min.y),
        Pos2::new(chips[0].max.x, chips[0].max.y),
    );
    assert!(row.width() > 0.0);

    let first_row = chips[0].min.y.min(chips[1].min.y);
    let second_row = tops[1];
    assert!(
        second_row - first_row >= BOARD_ROW_MIN_HEIGHT - height,
        "the second row of workers starts a whole row under the first: {second_row} against {first_row}"
    );
}

#[test]
fn the_head_row_of_the_grid_is_44_points_and_keeps_its_tag_in_a_square() {
    let shapes = pass(1400.0, &board_document());
    let top = f32::from(BODY_PAD_TOP);
    let label = painted(&shapes, "Understand").expect("the first wave is drawn");
    assert!(
        (label.center().y - (top + BOARD_HEAD_HEIGHT * 0.5)).abs() < 1.5,
        "the name of a wave stands in the middle of the head row: {label:?}"
    );
    let tag = painted(&shapes, "1").expect("the tag of the first wave is drawn");
    assert!(
        tag.height() <= 20.0 && tag.width() <= 20.0,
        "the tag of a wave stays inside its square of 20 points: {tag:?}"
    );
    let worker = painted(&shapes, "helper-1 · build-1").expect("the first worker is drawn");
    assert!(
        worker.center().y > top + BOARD_HEAD_HEIGHT,
        "the workers start under the head row: {worker:?}"
    );
}

#[test]
fn the_values_of_one_column_end_at_one_right_edge() {
    let shapes = pass(1400.0, &board_document());
    // Two items of the wave that runs share one column, so their values end at
    // one edge.
    let first = painted(&shapes, "8 m").expect("the value of the running item");
    let second = painted(&shapes, "~5 m").expect("the value of the waiting item");
    assert!(
        (first.max.x - second.max.x).abs() < 0.01,
        "the values of one column end at the same edge: {first:?} against {second:?}"
    );
    let head = painted(&shapes, "10 m").expect("the value of the wave");
    let chips = chips(&shapes);
    let column_right = chips
        .iter()
        .filter(|chip| chip.min.x <= head.center().x && head.center().x <= chip.max.x)
        .map(|chip| chip.max.x)
        .fold(f32::MIN, f32::max);
    assert!(
        head.max.x <= column_right && column_right - head.max.x <= 6.0,
        "the value of the head ends at the right edge of its column: {head:?} against {chips:?}"
    );
}

#[test]
fn the_narrow_board_names_the_place_of_every_item_and_folds_the_finished_waves() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Board {
            columns: vec![
                column("Understand", "1", "4 m", false),
                column("Prepare", "2", "9 m", false),
                column("Change", "3", "10 m", true),
            ],
            rows: vec![worker(1)],
            items: vec![
                item(0, 0, "Read the issue", BoardState::Done, "4 m", "1 failing test found"),
                item(1, 0, "Add the test", BoardState::Done, "6 m", "12 tests pass"),
                item(2, 0, "Change the target", BoardState::Now, "8 m", "edits routes"),
            ],
        }],
    };
    let shapes = pass(420.0, &document);

    assert!(
        painted_holding(&shapes, "3 · CHANGE · NOW"),
        "the caption of a wave names its tag, its name and that it runs now"
    );
    assert!(
        !painted_holding(&shapes, "UNDERSTAND") && !painted_holding(&shapes, "PREPARE"),
        "the captions of the folded waves keep no room on the card"
    );
    assert!(
        !painted_any(&shapes, "Read the issue"),
        "an item of a folded wave is not drawn until the person shows it"
    );
    assert!(
        painted_any(&shapes, "2 done"),
        "the fold line says how many items of the folded waves finished"
    );
    assert!(
        painted_any(&shapes, "show"),
        "the fold line offers to show the items again"
    );
    let running = painted(&shapes, "Change the target").expect("the wave that runs keeps its item");
    let note = painted(&shapes, "helper-1 · build-1 · edits routes").expect("the place of the item is on its line");
    assert!(
        note.min.y > running.min.y,
        "the place stands on the second line of the item: {note:?} against {running:?}"
    );
}

#[test]
fn a_press_on_the_fold_line_shows_the_items_and_a_second_press_folds_them() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Board {
            columns: vec![
                column("Understand", "1", "4 m", false),
                column("Change", "2", "10 m", true),
            ],
            rows: vec![worker(1)],
            items: vec![
                item(0, 0, "Read the issue", BoardState::Done, "4 m", "1 test"),
                item(1, 0, "Change the target", BoardState::Now, "8 m", "edits routes"),
            ],
        }],
    };
    let frames = CardFrames::new();
    let mut card = CardRuntime::default();
    let (folded, nodes) = frames.pass_in(420.0, &document, &mut card, Owner::Listening);
    assert!(painted_any(&folded, "3 done") || painted_any(&folded, "1 done"));
    let position = nodes
        .iter()
        .find_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            (text == "1 done").then(|| node.bounds()).flatten()
        })
        .expect("the fold line is a button of the card");
    let centre = Pos2::new(
        #[allow(clippy::cast_possible_truncation)]
        {
            f64::midpoint(position.x0, position.x1) as f32
        },
        #[allow(clippy::cast_possible_truncation)]
        {
            f64::midpoint(position.y0, position.y1) as f32
        },
    );
    let button = |pressed: bool| egui::Event::PointerButton {
        pos: centre,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let _ = frames.pass(
        &document,
        &mut card,
        Owner::Listening,
        vec![egui::Event::PointerMoved(centre)],
    );
    let _ = frames.pass(&document, &mut card, Owner::Listening, vec![button(true)]);
    let _ = frames.pass(&document, &mut card, Owner::Listening, vec![button(false)]);
    let (shown, _) = frames.pass_in(420.0, &document, &mut card, Owner::Listening);
    assert!(
        painted_any(&shown, "Read the issue"),
        "a press on the fold line shows the items of the folded waves"
    );
    assert!(
        painted_holding(&shown, "1 · UNDERSTAND"),
        "the shown wave keeps its caption"
    );
}

#[test]
fn the_callout_is_one_row_in_the_wide_form_and_a_stack_in_the_narrow_one() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Callout {
            tone: Tone::Warn,
            caption: Some("Needs you · t5 · waits 3 min".to_owned()),
            text: "Which address must the redirect use?".to_owned(),
            buttons: vec![
                Button {
                    id: "answer.account".to_owned(),
                    label: "The account address".to_owned(),
                    tone: Tone::Primary,
                },
                Button {
                    id: "answer.last".to_owned(),
                    label: "The last address".to_owned(),
                    tone: Tone::Neutral,
                },
            ],
        }],
    };
    let wide = pass(1400.0, &document);
    let caption = painted(&wide, "NEEDS YOU · T5 · WAITS 3 MIN").expect("the caption of the callout is drawn");
    let question = painted(&wide, "Which address must the redirect use?").expect("the question is drawn");
    let answer = painted(&wide, "The account address").expect("the answers are drawn");
    assert!(
        caption.min.y < question.min.y,
        "the caption stands above the question: {caption:?} against {question:?}"
    );
    assert!(
        answer.min.x > question.max.x,
        "the answers keep the right end of the row: {answer:?} against {question:?}"
    );
    assert!(
        (answer.center().y - question.center().y).abs() < 12.0,
        "the answers share the row of the question: {answer:?} against {question:?}"
    );

    let narrow = pass(420.0, &document);
    let question = painted(&narrow, "Which address must the redirect use?").expect("the question is drawn");
    let account = painted(&narrow, "The account address").expect("the first answer is drawn");
    let last = painted(&narrow, "The last address").expect("the second answer is drawn");
    assert!(
        account.min.y > question.max.y,
        "the answers stand under the question: {account:?} against {question:?}"
    );
    assert!(
        last.min.y > account.min.y,
        "the answers of the narrow form stand one under another: {last:?} against {account:?}"
    );
    assert!(
        (account.min.x - last.min.x).abs() < 10.0,
        "the answers of the narrow form keep one left edge: {account:?} against {last:?}"
    );
    let frame = CALLOUT_PAD_Y;
    assert!(
        question.min.y - caption.min.y >= frame,
        "the caption keeps the padding of the frame: {caption:?}"
    );
}

#[test]
fn a_list_that_offers_no_selection_draws_no_ring_and_keeps_one_right_edge() {
    let document = BlockDocument {
        width: None,
        components: vec![Component::List {
            selection: Selection::None,
            rows: vec![
                horizon_browser_control::block::ListRow {
                    id: "row.1".to_owned(),
                    title: "Short".to_owned(),
                    subtitle: None,
                    tone: None,
                    selected: false,
                    value: Some("1. min".to_owned()),
                    badge: None,
                    disabled: false,
                },
                horizon_browser_control::block::ListRow {
                    id: "row.2".to_owned(),
                    title: "A row whose second line needs two lines of the card".to_owned(),
                    subtitle: Some("with a reason that takes the room that it needs".to_owned()),
                    tone: None,
                    selected: false,
                    value: Some("22 min".to_owned()),
                    badge: None,
                    disabled: false,
                },
            ],
            layout: ListLayout::default(),
        }],
    };
    let shapes = pass(420.0, &document);
    let rings = shapes
        .iter()
        .filter(|clipped| matches!(&clipped.shape, Shape::Circle(circle) if circle.stroke.width > 0.0))
        .count();
    assert_eq!(rings, 0, "a list that offers no selection draws no ring");

    let first = painted(&shapes, "1. min").expect("the first value is drawn");
    let second = painted(&shapes, "22 min").expect("the second value is drawn");
    assert!(
        (first.max.x - second.max.x).abs() < 0.01,
        "the values of one list end at one right edge: {first:?} against {second:?}"
    );
    assert!(
        (first.min.x - second.min.x).abs() < 0.01,
        "the values of one list start at one left edge: {first:?} against {second:?}"
    );
    let title = painted(&shapes, "A row whose second line needs two lines of the card").map_or(0.0, |rect| rect.max.x);
    assert!(
        title <= first.min.x,
        "the text of a row keeps the room that the ring took: {title} against {}",
        first.min.x
    );
}

#[test]
fn the_log_flows_into_columns_of_at_least_330_points_in_the_wide_form_only() {
    let entries: Vec<LogEntry> = (0..5)
        .map(|index| LogEntry {
            at: format!("07:3{index}"),
            text: format!("t{index} done on build-2, 9 rows"),
        })
        .collect();
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Log {
            entries: entries.clone(),
        }],
    };
    let wide = pass(1400.0, &document);
    let first = painted(&wide, "t0 done on build-2, 9 rows").expect("the first entry is drawn");
    let second = painted(&wide, "t1 done on build-2, 9 rows").expect("the second entry is drawn");
    let third = painted(&wide, "t2 done on build-2, 9 rows").expect("the third entry is drawn");
    assert!(
        (first.min.y - second.min.y).abs() < 0.01 && second.min.x > first.min.x,
        "the entries of the wide form flow row by row: {first:?} {second:?}"
    );
    assert!(
        third.min.x > second.min.x,
        "one row holds as many entries as the columns of at least 330 points: {third:?} against {second:?}"
    );
    assert!(
        (second.min.x - first.min.x) >= 330.0,
        "a column of the log is at least 330 points wide: {}",
        second.min.x - first.min.x
    );

    let narrow = pass(420.0, &document);
    let first = painted(&narrow, "t0 done on build-2, 9 rows").expect("the first entry is drawn");
    let second = painted(&narrow, "t1 done on build-2, 9 rows").expect("the second entry is drawn");
    assert!(
        second.min.y > first.min.y && (second.min.x - first.min.x).abs() < 0.01,
        "the narrow form keeps one column: {first:?} {second:?}"
    );
}

#[test]
fn a_wide_card_opens_at_half_of_the_canvas_view_between_its_bounds() {
    let document = board_document();
    assert_eq!(default_card_size(&document, 5120.0)[0], 2400.0);
    assert_eq!(default_card_size(&document, 2000.0)[0], 1100.0);
    assert_eq!(default_card_size(&document, 20_000.0)[0], 2400.0);
    assert_eq!(
        default_card_size(
            &BlockDocument {
                width: Some(Width::Narrow),
                components: vec![board()],
            },
            5120.0
        )[0],
        420.0,
        "a document that asks for the narrow card keeps the card of today"
    );
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
fn a_wide_card_takes_a_height_up_to_1200_points_and_a_narrow_one_up_to_900() {
    assert_eq!(card_height_limits(&board_document()), [CARD_HEIGHT_LIMITS[0], 1200.0]);
    assert_eq!(
        card_height_limits(&BlockDocument {
            width: None,
            components: vec![],
        }),
        CARD_HEIGHT_LIMITS
    );
    assert_eq!(
        fitted_card_height(
            4000.0,
            false,
            CARD_HEIGHT_LIMITS[0],
            card_height_limits(&board_document())
        ),
        1200.0
    );
}

#[test]
fn the_estimate_of_a_board_a_callout_and_a_log_grows_with_their_parts_in_both_forms() {
    let document = BlockDocument {
        width: Some(Width::Wide),
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
        // body, which the estimate of the card also holds.
        let estimated = size[1] - crate::block_widget::CARD_CHROME_HEIGHT;
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
            components: vec![component.clone()],
        };
        let two = BlockDocument {
            width: None,
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
    // The board paints the accent of the current theme, so the same document
    // follows the light theme without a branch of its own.
    let _theme = crate::block_widget::tests::theme_lock();
    let previous = crate::theme::current_theme();
    let dark = {
        crate::theme::set_theme(crate::theme::ResolvedTheme::Dark);
        pass(1400.0, &board_document())
    };
    crate::theme::set_theme(crate::theme::ResolvedTheme::Light);
    let light = pass(1400.0, &board_document());
    crate::theme::set_theme(previous);

    assert_ne!(
        outline_colours(&dark),
        outline_colours(&light),
        "the two themes draw other colours"
    );
}

/// The stroke colours of every outlined rectangle of a pass.
fn outline_colours(shapes: &[ClippedShape]) -> Vec<Color32> {
    shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(rect.stroke.color),
            _ => None,
        })
        .collect()
}

/// The size of the glyphs that the pass asked for when it painted `text`.
fn painted_font_size(shapes: &[ClippedShape], text: &str) -> f32 {
    shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text() == text => Some(shape.galley.job.sections[0].format.font_id.size),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the card drew {text}"))
}

/// A document of a heading that a progress bar follows directly.
fn heading_and_progress() -> BlockDocument {
    BlockDocument {
        width: Some(Width::Wide),
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

#[test]
fn a_progress_bar_that_follows_a_heading_shares_its_row_in_the_wide_form_only() {
    let wide = pass(1400.0, &heading_and_progress());
    let heading = painted(&wide, "Fix the login redirect").expect("the heading is drawn");
    let label = painted(&wide, "3 of 8 done").expect("the label of the bar is drawn");
    let detail = painted(&wide, "about 30 min left").expect("the detail of the bar is drawn");

    assert!(
        (painted_font_size(&wide, "Fix the login redirect") - 26.0).abs() < f32::EPSILON,
        "the heading of that row takes the larger title"
    );
    let body_right = 1400.0 - f32::from(BODY_PAD_SIDE);
    assert!(
        (label.min.x - (body_right - 320.0)).abs() < 1.5 && label.min.y < heading.max.y,
        "the bar stands at the right of the heading row: {label:?} against {heading:?}"
    );
    assert!(
        detail.max.x <= body_right + 0.5 && detail.max.x > body_right - 1.5,
        "the detail keeps the right edge of the bar: {detail:?}"
    );

    let narrow = pass(420.0, &heading_and_progress());
    let heading = painted(&narrow, "Fix the login redirect").expect("the heading is drawn");
    let label = painted(&narrow, "3 of 8 done").expect("the label of the bar is drawn");
    assert!(
        (painted_font_size(&narrow, "Fix the login redirect") - 18.0).abs() < f32::EPSILON,
        "the narrow card keeps the title of today"
    );
    assert!(
        label.min.y > heading.max.y,
        "the bar takes its own row under the heading: {label:?} against {heading:?}"
    );
}

#[test]
fn the_buttons_of_a_wide_card_close_their_row_at_the_right_edge() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![Component::Buttons {
            items: vec![
                Button {
                    id: "plan".to_owned(),
                    label: "Change the plan".to_owned(),
                    tone: Tone::Neutral,
                },
                Button {
                    id: "stop".to_owned(),
                    label: "Stop".to_owned(),
                    tone: Tone::Danger,
                },
            ],
        }],
    };
    let wide = pass(1400.0, &document);
    let stop = painted(&wide, "Stop").expect("the last button is drawn");
    let plan = painted(&wide, "Change the plan").expect("the first button is drawn");
    assert!(
        plan.min.x > 400.0,
        "the buttons of a wide card keep the right end of the row: {plan:?}"
    );
    assert!(
        (stop.max.x - (1400.0 - f32::from(BODY_PAD_SIDE) - 14.0)).abs() < 1.0,
        "the last button ends at the right edge of the body, less its padding: {stop:?}"
    );
    assert!(
        (plan.center().y - stop.center().y).abs() < 0.01,
        "the buttons of the wide form share one row"
    );

    let narrow = pass(420.0, &document);
    let plan = painted(&narrow, "Change the plan").expect("the first button is drawn");
    assert!(
        (plan.min.x - (f32::from(BODY_PAD_SIDE) + 14.0)).abs() < 1.0,
        "the narrow card keeps its buttons at the left edge: {plan:?}"
    );
}
