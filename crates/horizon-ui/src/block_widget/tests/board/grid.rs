//! The grid of the wide form: its columns, its head row, its items and the room
//! that it needs.

use super::*;

use crate::block_widget::metrics::{
    BOARD_CELL_PAD, BOARD_COLUMN_MAX_WIDTH, BOARD_HEAD_COLUMN, BOARD_HEAD_GAP, BOARD_HEAD_HEIGHT, BOARD_HEAD_PAD_SIDE,
    BOARD_ROW_MIN_HEIGHT, BOARD_TAG_SIZE, BODY_PAD_SIDE, BODY_PAD_TOP, BUTTON_PAD, RADIUS_FRAME,
};

#[test]
fn the_grid_keeps_one_line_of_evidence_inside_the_item() {
    // A card of 1100 points with three waves: the note of the item is wider
    // than its cell, so the grid has to cut it.
    let short = chips(&pass(1100.0, &one_long_note("edits routes")));
    let shapes = pass(
        1100.0,
        &one_long_note("your push is your own choice, and nobody else reads it"),
    );
    let chips = chips(&shapes);
    assert_eq!(chips.len(), 1, "the board draws one item");
    assert!(
        (chips[0].height() - short[0].height()).abs() < 0.01,
        "a long note keeps the height of the item: {:?} against {:?}",
        chips[0],
        short[0]
    );

    let (note, lines, last) = painted_text(&shapes, "your push is your own choice");
    assert_eq!(lines, 1, "the grid keeps one line of evidence");
    assert!(
        last.ends_with('\u{2026}'),
        "the line of the evidence ends with an ellipsis: {last:?}"
    );
    assert!(
        chips[0].contains_rect(note),
        "the evidence stays inside its item: {note:?} against {:?}",
        chips[0]
    );
}

#[test]
fn the_grid_gives_every_wave_the_same_width_after_the_230_points_of_the_workers() {
    let shapes = pass(1400.0, &board_document());
    let understand = painted(&shapes, "Understand").expect("the first wave is drawn");
    let change = painted(&shapes, "Change").expect("the second wave is drawn");
    let verify = painted(&shapes, "Verify").expect("the third wave is drawn");
    let column_width = (body(1400.0) - BOARD_HEAD_COLUMN) / 3.0;
    let first_column =
        f32::from(BODY_PAD_SIDE) + BOARD_HEAD_COLUMN + BOARD_HEAD_PAD_SIDE + BOARD_TAG_SIZE + BOARD_HEAD_GAP;

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
fn a_column_of_the_grid_is_at_most_480_points_and_the_rest_of_the_card_keeps_its_width() {
    // A card of 2400 points: the person made it wide, and two waves keep the
    // left edge in columns of 480 points instead of stretching over the card.
    let document = board_with_a_callout_a_log_and_a_button();
    let shapes = pass(2400.0, &document);
    let body_left = f32::from(BODY_PAD_SIDE);

    // The callout keeps the full width of the body, not the width of the grid.
    let question =
        painted(&shapes, "Which address must the redirect use?").expect("the question of the callout is drawn");
    let callout = shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Rect(rect)
                if rect.corner_radius == CornerRadius::same(RADIUS_FRAME) && rect.rect.contains_rect(question) =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("the callout draws its frame");
    let chips: Vec<Rect> = chips(&shapes)
        .into_iter()
        .filter(|chip| chip.max.y <= callout.min.y)
        .collect();
    assert_eq!(chips.len(), 2, "the grid draws the item of every wave");
    let item_width = BOARD_COLUMN_MAX_WIDTH - 2.0 * BOARD_CELL_PAD;
    assert!(
        chips.iter().all(|chip| (chip.width() - item_width).abs() < 0.01),
        "a column of the grid is 480 points wide, so its item is {item_width}: {chips:?}"
    );
    assert!(
        (chips[0].min.x - (body_left + BOARD_HEAD_COLUMN + BOARD_CELL_PAD)).abs() < 0.01
            && (chips[0].max.x - (body_left + BOARD_HEAD_COLUMN + BOARD_COLUMN_MAX_WIDTH - BOARD_CELL_PAD)).abs()
                < 0.01,
        "the grid starts at the left edge of the body: {:?}",
        chips[0]
    );
    assert!(
        (callout.min.x - body_left).abs() < 0.01 && (callout.max.x - (2400.0 - body_left)).abs() < 1.5,
        "the callout keeps the width of the body: {callout:?}"
    );

    // The log flows into the six columns that the body holds, so the six
    // entries of one row stand at one height.
    let first = painted(&shapes, "t0 done on build-2, 9 rows").expect("the first entry is drawn");
    for (index, at) in [(1, "t1 done on build-2, 9 rows"), (5, "t5 done on build-2, 9 rows")] {
        let entry = painted(&shapes, at).expect("the entry is drawn");
        assert!(
            (entry.min.y - first.min.y).abs() < 0.01 && entry.min.x > first.min.x,
            "entry {index} shares the row of the log and the width of the body: {entry:?} against {first:?}"
        );
    }

    let stop = painted(&shapes, "Stop").expect("the button is drawn");
    assert!(
        (stop.max.x - (2400.0 - body_left - BUTTON_PAD)).abs() < 1.0,
        "the button keeps the right edge of the body: {stop:?}"
    );
}

#[test]
fn the_board_draws_the_list_when_a_column_of_the_grid_would_be_narrower_than_260_points() {
    // Seven waves need 230 + 7 x 260 = 2050 points of body, and a card of 1600
    // points gives 1568, so the card draws the list of the narrow form.
    let narrow = pass(1600.0, &board_of(7));
    assert!(
        painted_holding(&narrow, "1 · WAVE 1 · NOW"),
        "the card draws the list of its board"
    );
    assert!(!painted_any(&narrow, "Wave 1"), "the grid keeps no room on the card");
    let chips = chips(&narrow);
    assert_eq!(chips.len(), 7, "the list draws every item");
    assert!(
        chips.iter().all(|chip| chip.width() <= 560.0),
        "the list keeps a largest width of 560 points: {chips:?}"
    );
    assert!(
        (chips[0].min.x - f32::from(BODY_PAD_SIDE)).abs() < 0.01,
        "the list stays at the left edge of the body: {:?}",
        chips[0]
    );

    // Three waves need 1010 points of body, so a card of 1702 points draws the
    // grid: every column keeps at least 260 points.
    let wide = pass(1702.0, &board_of(3));
    assert!(painted_any(&wide, "Wave 1"), "the card draws the grid of its board");
    assert!(!painted_holding(&wide, "1 · WAVE 1 · NOW"));
}
