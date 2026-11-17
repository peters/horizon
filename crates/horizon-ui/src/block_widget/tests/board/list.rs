//! The list of the narrow form: the place of every item on its line, the fold of
//! the finished waves, and the two lines that a note may take.

use super::*;

#[test]
fn the_list_of_the_narrow_form_gives_a_note_two_lines_and_then_an_ellipsis() {
    let note = "your push is your own choice, and nobody else reads it, so the branch stays yours";
    let shapes = pass(420.0, &one_long_note_narrow(note));
    let item_rects = chips(&shapes);
    assert_eq!(item_rects.len(), 1, "the board draws one item");

    let (evidence, lines, last) = painted_text(&shapes, "helper-1 \u{b7} build-1 \u{b7} your push");
    assert_eq!(lines, 2, "a note of the narrow form takes two lines of the card");
    assert!(
        last.ends_with('\u{2026}'),
        "the second line ends with an ellipsis: {last:?}"
    );
    assert!(
        item_rects[0].contains_rect(evidence),
        "the evidence stays inside its item: {evidence:?} against {:?}",
        item_rects[0]
    );

    // No note takes a third line, whatever its length.
    let longer = one_long_note_narrow(&format!("{note}, and the notes of the run stay on this card"));
    let taller = pass(420.0, &longer);
    let (_, lines, _) = painted_text(&taller, "helper-1 \u{b7} build-1 \u{b7} your push");
    assert_eq!(lines, 2, "the longest note keeps the two lines of the list");
    assert_eq!(
        chips(&taller)[0].height(),
        item_rects[0].height(),
        "and the height of the item"
    );
}

#[test]
fn the_narrow_board_names_the_place_of_every_item_and_folds_the_finished_waves() {
    let document = BlockDocument {
        width: Some(Width::Wide),
        finished: false,
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
        finished: false,
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
