//! The row of a proposal of the front desk and the reference row of a work item:
//! the rectangles that they draw, the press that starts a proposal, and the
//! proposal that a restart loaded as open, which takes no press.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::agent_bridge::{Entry, ProposalItem, ProposalState};

use crate::app::orchestrator_dock::list;
use crate::app::orchestrator_dock::proposal;
use crate::app::orchestrator_dock::tests::chat::stand_in;
use crate::app::orchestrator_dock::tests::frame::{Drawn, Window, rounded_rects, texts};
use crate::app::orchestrator_dock::tests::front_desk::WIDE;
use crate::app::orchestrator_dock::{FRONT_DESK, OrchestratorDock};
use crate::app::sidebar::inbox::asking_card;

/// The stand-in of the program of the work item of one test: it names its session
/// and then reads the lines of the person until the bridge stops.
const SILENT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while IFS= read -r line; do :; done
"#;

/// The items of the proposal of these tests.
fn items() -> Vec<ProposalItem> {
    vec![
        ProposalItem {
            title: "Fix login redirect".to_owned(),
            task: "Fix issue 1987. Done means the test passes.".to_owned(),
            folder: Some(format!("/home/person/{}work", "a/very/long/folder/".repeat(10))),
        },
        ProposalItem {
            title: "Faster cold start".to_owned(),
            task: "question: make the cold start faster.".to_owned(),
            folder: None,
        },
    ]
}

/// A window whose dock shows the front desk, with the switch of the dock off, so no
/// program of a conversation starts on its own.
fn window() -> Window {
    let mut window = Window::new(WIDE, Vec::new(), true);
    window.frame(Vec::new());
    window
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// One press of the person on `rect`.
fn press(window: &mut Window, rect: Rect) {
    let at = rect.center();
    window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    window.frame(vec![button(at, false)]);
}

/// The one text of `drawn` that reads `text`.
fn drawn_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

/// Whether `drawn` holds the text `text`.
fn draws(drawn: &[Drawn], text: &str) -> bool {
    drawn.iter().any(|drawn| drawn.text == text)
}

#[test]
fn the_row_of_a_proposal_draws_its_caption_its_items_and_its_two_controls() {
    let mut window = window();
    let app = window.app();
    app.orchestrator_dock.propose("p-1", &items());
    let drawn = texts(&window.frame(Vec::new()));

    let body = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .body;
    let caption = drawn_text(&drawn, proposal::CAPTION);
    let first_title = drawn_text(&drawn, "Fix login redirect");
    let second_title = drawn_text(&drawn, "Faster cold start");
    let start = drawn_text(&drawn, "Start 2 work items");
    let decline = drawn_text(&drawn, proposal::DECLINE);

    // The row stands at the left of the list, 13 points inside it, and its text 12
    // points inside the box.
    let left = body.min.x + list::LIST_PADDING + list::CARD_INDENT + proposal::PADDING;
    assert!(
        (caption.rect.min.x - left).abs() < 0.5,
        "the caption stands at the padding of the box: {} against {left}",
        caption.rect.min.x
    );
    assert!(
        (first_title.rect.min.x - left).abs() < 0.5,
        "the title of the first item stands at the same left edge"
    );
    assert_eq!(
        second_title.rect.min.x, first_title.rect.min.x,
        "every item of a proposal starts at the same left edge"
    );
    assert!(
        caption.rect.max.y <= first_title.rect.min.y,
        "the caption stands above the first item"
    );
    assert!(
        first_title.rect.max.y <= second_title.rect.min.y,
        "the second item stands under the first"
    );
    // The two controls stand on one line, under the items, 8 points apart.
    assert!(
        second_title.rect.max.y <= start.rect.min.y,
        "the controls stand under the last item"
    );
    assert_eq!(
        start.rect.center().y,
        decline.rect.center().y,
        "the two controls stand on one line"
    );
    assert!(
        (decline.rect.min.x - start.rect.max.x - 2.0 * proposal::BUTTON_PADDING - proposal::BUTTON_GAP).abs() < 0.5,
        "12 points of padding and 8 points of air stand between the two labels: {} against {}",
        decline.rect.min.x,
        start.rect.max.x
    );
    assert!(
        start.rect.height() < proposal::BUTTON_HEIGHT,
        "a label of 12 points stands inside a control of 28 points"
    );
    // The folder of the first item shows its end.
    let folder = drawn
        .iter()
        .find(|drawn| drawn.text.starts_with('\u{2026}') && drawn.text.ends_with("work"))
        .cloned()
        .unwrap_or_else(|| panic!("the folder of an item shows its end: {drawn:?}"));
    assert!(folder.rect.min.x >= left, "the folder of an item stands inside the box");
    assert!(
        folder.rect.max.x <= body.max.x,
        "the folder of an item ends inside the list"
    );
}

#[test]
fn a_press_on_the_control_of_a_proposal_starts_its_work_items() {
    let mut window = window();
    window.app().orchestrator_dock.propose("p-1", &items());
    let drawn = texts(&window.frame(Vec::new()));
    let start = drawn_text(&drawn, "Start 2 work items");
    press(&mut window, start.rect);

    // The frame that reads the answer of the check starts one workspace for each
    // item.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        window.frame(Vec::new());
        let names: Vec<String> = window
            .app()
            .board
            .workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect();
        if names.contains(&"Fix login redirect".to_owned()) && names.contains(&"Faster cold start".to_owned()) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the press of the row starts the work items: {names:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, proposal::STARTED),
        "the row of the proposal that the person started says Started: {drawn:?}"
    );
    assert!(
        !draws(&drawn, "Start 2 work items"),
        "a proposal that the person started takes no second press"
    );
}

#[test]
fn a_proposal_that_a_restart_loaded_as_open_reads_not_started_and_takes_no_press() {
    let mut window = window();
    let proposals = vec![Entry::Proposal {
        id: "loaded".to_owned(),
        items: items(),
        state: ProposalState::Open,
    }];
    window.app().orchestrator_dock.load_for_test(FRONT_DESK, proposals);
    assert_eq!(
        window.app().orchestrator_dock.loaded_entries(FRONT_DESK),
        1,
        "the proposal came from the file"
    );
    // The conversation of the front desk now holds the stored proposal alone, so
    // the list draws it.
    let drawn = texts(&window.frame(Vec::new()));
    assert_eq!(
        window.app().orchestrator_dock.loaded_entries(FRONT_DESK),
        1,
        "the frame keeps the mark of the loaded entries"
    );
    assert!(
        draws(&drawn, proposal::NOT_STARTED),
        "a proposal of an earlier run reads Not started: {drawn:?}"
    );
    assert!(
        !draws(&drawn, "Start 2 work items"),
        "the program that proposed it is gone, so the row draws no control: {drawn:?}"
    );
}

#[test]
fn a_message_that_the_front_desk_passed_carries_its_caption_above_the_bubble() {
    let mut window = window();
    window.app().orchestrator_dock.load_for_test(
        FRONT_DESK,
        vec![Entry::Person {
            text: "the branch is pushed".to_owned(),
            via_front_desk: true,
        }],
    );
    let drawn = texts(&window.frame(Vec::new()));
    let caption = drawn_text(&drawn, "via the front desk");
    let bubble_text = drawn_text(&drawn, "the branch is pushed");
    assert!(
        caption.rect.max.y <= bubble_text.rect.min.y,
        "the caption stands above the bubble: {:?} against {:?}",
        caption.rect,
        bubble_text.rect
    );
    // The caption is right aligned with the right edge of the bubble, which stands
    // one padding right of the text inside it.
    let bubble_right = bubble_text.rect.max.x + list::BUBBLE_PADDING_X;
    assert!(
        (caption.rect.max.x - bubble_right).abs() < 0.5,
        "the caption ends at the right edge of the bubble: {} against {bubble_right}",
        caption.rect.max.x
    );
    assert_eq!(
        caption.color,
        Some(crate::theme::FG_DIM()),
        "the caption of a passed message reads dim"
    );
}

#[test]
fn the_reference_row_of_a_work_item_draws_its_mark_its_name_and_its_state() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = stand_in(folder.path(), SILENT);
    let mut window = Window::with_program(WIDE, Vec::new(), &program);
    let workspace = window.app().board.create_workspace("Faster cold start");
    let _ = window.frame(Vec::new());
    let local_id = window
        .app()
        .board
        .workspace(workspace)
        .map(|workspace| workspace.local_id.clone())
        .expect("the workspace of the test");
    // The orchestrator of that work item runs, so a card of the work item asks a
    // question that the person reads as a state of the row.
    let ctx = window.ctx().clone();
    assert!(
        window.app().say_to_orchestrator(&ctx, &local_id, "hello"),
        "the message of the person reaches the conversation of the work item"
    );
    window.wait_for("the program of the work item", |app| {
        app.orchestrator_dock.running(&local_id)
    });
    window.app().orchestrator_dock.front_desk_mut().show();
    window
        .app()
        .orchestrator_dock
        .work_item_row(&local_id, "Faster cold start");
    asking_card(window.app(), workspace, "Which route?", &[("go", "Go")]);
    window.app().sidebar_list.count_inbox_change();
    window.wait_for("the state of the work item", |app| {
        app.orchestrator_dock.work_item_word(&local_id) == Some("needs you")
    });
    let drawn = texts(&window.frame(Vec::new()));

    let body = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .body;
    let name = drawn_text(&drawn, "Faster cold start");
    let state = drawn_text(&drawn, "needs you");
    assert!(
        !draws(&drawn, "needs_you"),
        "the row of a work item says no machine word: {drawn:?}"
    );
    // The mark of the workspace comes first, then its name 10 points after it.
    let tile_left = body.min.x + list::LIST_PADDING + list::CARD_INDENT + list::CARD_PADDING_X;
    assert!(
        (name.rect.min.x - (tile_left + 20.0 + list::CARD_GAP_AFTER_GLYPH)).abs() < 0.5,
        "the name stands after the mark of 20 points: {} against {tile_left}",
        name.rect.min.x
    );
    // The state word ends at the right padding of the row.
    let row_right = name.rect.min.x + (body.width() - 2.0 * list::LIST_PADDING) - list::CARD_INDENT;
    assert!(
        state.rect.max.x <= row_right && state.rect.min.x > name.rect.max.x,
        "the state word stands at the right end of the row: {:?} against {row_right}",
        state.rect
    );
    assert_eq!(
        name.rect.center().y,
        state.rect.center().y,
        "the name and the state word stand on one line"
    );
}

#[test]
fn the_reference_row_of_a_work_item_that_left_the_board_reads_dim_and_takes_no_press() {
    let mut window = window();
    window
        .app()
        .orchestrator_dock
        .work_item_row("11111111-2222-4333-8444-555555555555", "A work item that is gone");
    let drawn = texts(&window.frame(Vec::new()));
    let name = drawn_text(&drawn, "A work item that is gone");
    assert_eq!(
        name.color,
        Some(crate::theme::FG_DIM()),
        "the row of a work item that the board does not hold reads dim"
    );
    let before = window.app().board.workspaces.len();
    press(&mut window, name.rect);
    assert_eq!(
        window.app().board.workspaces.len(),
        before,
        "a press on the row of a work item that is gone changes nothing"
    );
    let _ = OrchestratorDock::default();
}

#[test]
fn the_reference_rows_of_two_work_items_stand_six_points_apart() {
    let mut window = window();
    let first = window.app().board.create_workspace("Faster cold start");
    let second = window.app().board.create_workspace("Fix login redirect");
    let _ = window.frame(Vec::new());
    window.app().orchestrator_dock.front_desk_mut().show();
    let app = window.app();
    for (workspace, name) in [(first, "Faster cold start"), (second, "Fix login redirect")] {
        let local_id = app
            .board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone())
            .expect("the workspace of the test");
        app.orchestrator_dock.work_item_row(&local_id, name);
    }
    app.orchestrator_dock.propose("p-1", &items());

    let output = window.frame(Vec::new());
    let body = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .body;
    let rows: Vec<Rect> = rounded_rects(&output, body, list::CARD_RADIUS)
        .into_iter()
        .filter(|rect| (rect.height() - list::CARD_HEIGHT).abs() < 0.5)
        .collect();
    assert_eq!(rows.len(), 2, "the list draws one row for each work item: {rows:?}");
    assert_eq!(
        rows[1].min.y - rows[0].max.y,
        list::REF_GAP,
        "two reference rows that follow each other stand 6 points apart: {rows:?}"
    );
    // The proposal under them is an entry of another kind, so it stands one gap of
    // the list away from the group.
    let caption = drawn_text(&texts(&output), proposal::CAPTION);
    let top = caption.rect.min.y - proposal::PADDING;
    assert!(
        (top - rows[1].max.y - list::ENTRY_GAP).abs() < 0.5,
        "an entry of another kind stands {} points from a reference row: {} against {}",
        list::ENTRY_GAP,
        top - rows[1].max.y,
        rows[1].max.y
    );
}
