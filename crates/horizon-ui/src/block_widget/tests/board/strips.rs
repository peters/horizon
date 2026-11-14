//! The parts beside the board: the callout, the list of a document, the log, the
//! progress bar of a heading row and the buttons of a wide card.

use horizon_browser_control::block::{ListLayout, Selection};

use super::*;
use crate::block_widget::metrics::{BUTTON_PAD, CALLOUT_PAD_Y, LINE_FACTOR, PROGRESS_BAR_HEIGHT, PROGRESS_LABEL_FONT};

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
    let frame = narrow
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Rect(rect) if rect.corner_radius == CornerRadius::same(RADIUS_FRAME) => Some(rect.rect),
            _ => None,
        })
        .expect("the callout draws its frame");
    assert!(
        question.min.y - frame.min.y >= CALLOUT_PAD_Y - 0.5 && last.max.y <= frame.max.y + 0.5,
        "the text of the callout keeps the padding of its frame: {question:?} in {frame:?}"
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
fn the_detail_of_a_wide_bar_takes_its_own_line_under_the_bar() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        components: vec![
            Component::Heading {
                text: "Fix the login redirect".to_owned(),
                subtitle: None,
                badge: None,
            },
            Component::Progress {
                label: "3 of 8 done · 1 needs you".to_owned(),
                percent: 38,
                detail: Some("Checks pass on both computers · 27 min so far · about 30 left".to_owned()),
            },
        ],
    };
    let wide = pass(1400.0, &document);
    let label = painted(&wide, "3 of 8 done · 1 needs you").expect("the label of the bar is drawn");
    let detail = painted(&wide, "Checks pass on both computers · 27 min so far · about 30 left")
        .expect("the detail of the bar is drawn");
    let body_right = 1400.0 - f32::from(BODY_PAD_SIDE);

    assert!(
        (detail.max.x - body_right).abs() < 1.5,
        "the detail keeps the right edge of the bar: {detail:?}"
    );
    assert!(
        detail.min.y > label.max.y + PROGRESS_BAR_HEIGHT,
        "the detail takes its own line under the bar: {detail:?} against {label:?}"
    );
    assert!(
        detail.height() < PROGRESS_LABEL_FONT * LINE_FACTOR + 1.0,
        "the detail is one line: {detail:?}"
    );
    assert!(
        (label.min.x - (body_right - 320.0)).abs() < 1.5,
        "the label keeps the row above the bar, at the left edge of the bar: {label:?}"
    );
    assert!(
        label.max.x <= detail.max.x + 1.5,
        "the label is above the bar, not beside the detail: {label:?} {detail:?}"
    );

    // Two more parts, so that the card is above its least height and the estimate
    // of the row of the heading and the bar is what decides the difference.
    let filler = "the person reads the board of the plan and the evidence of each todo";
    let mut document = document;
    document.components.push(Component::Text {
        text: filler.to_owned(),
    });
    document.components.push(Component::Text {
        text: filler.to_owned(),
    });
    let mut without_detail = document.clone();
    if let Some(Component::Progress { detail, .. }) = without_detail.components.get_mut(1) {
        *detail = None;
    }
    let [wide, with_line] = default_card_size(&document, 3000.0);
    let [_, without_line] = default_card_size(&without_detail, 3000.0);
    assert!(
        with_line > without_line,
        "the height estimate of a card {wide} points wide counts the line of the detail: {with_line} against {without_line}"
    );

    let narrow = pass(420.0, &document);
    let label = painted(&narrow, "3 of 8 done · 1 needs you").expect("the label of the narrow bar");
    let detail = painted(&narrow, "Checks pass on both computers · 27 min so far · about 30 left")
        .expect("the detail of the narrow bar");
    assert!(
        (detail.min.y - label.min.y).abs() < 1.5 && detail.min.x > label.min.x,
        "the narrow card keeps the label and the detail on one line: {label:?} {detail:?}"
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
        (stop.max.x - (1400.0 - f32::from(BODY_PAD_SIDE) - BUTTON_PAD)).abs() < 1.0,
        "the last button ends at the right edge of the body, less its padding: {stop:?}"
    );
    assert!(
        (plan.center().y - stop.center().y).abs() < 0.01,
        "the buttons of the wide form share one row"
    );

    let narrow = pass(420.0, &document);
    let plan = painted(&narrow, "Change the plan").expect("the first button is drawn");
    assert!(
        (plan.min.x - (f32::from(BODY_PAD_SIDE) + BUTTON_PAD)).abs() < 1.0,
        "the narrow card keeps its buttons at the left edge: {plan:?}"
    );
}
