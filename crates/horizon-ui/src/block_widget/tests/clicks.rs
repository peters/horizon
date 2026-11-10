//! A press reaches the card runtime exactly as it did before the redesign: one
//! click per press, with the row selection that was visible at that moment.

use horizon_browser_control::block::Component;
use horizon_core::CardRuntime;

use super::{CardFrames, Owner, every_component, one_button};

#[test]
fn a_press_on_a_button_records_a_click_with_the_revision_the_person_saw() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, Owner::Listening, "Connect");

    let click = card.take_click(1_000).expect("a press on a live button is a click");
    assert_eq!(click.component_id, "connect");
    assert_eq!(click.revision, 3);
    assert!(click.selected.is_empty(), "no row was selected");
}

#[test]
fn a_press_on_a_row_selects_it_and_is_a_click_with_the_selection() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, Owner::Listening, "Google Cloud");

    let click = card.take_click(1_000).expect("a press on a live row is a click");
    assert_eq!(click.component_id, "gcloud");
    assert_eq!(click.selected, vec!["gcloud".to_owned()]);
    assert_eq!(card.selected(6), Some("gcloud"));
}

#[test]
fn a_press_records_nothing_while_the_agent_is_away() {
    let document = one_button();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, Owner::Gone, "Connect");

    assert!(
        card.take_click(1_000).is_none(),
        "a card without its agent takes no click"
    );
}

#[test]
fn a_press_on_a_dangerous_button_is_a_click_like_any_other() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, Owner::Listening, "Delete");

    let click = card.take_click(1_000).expect("a press on Delete is a click");
    assert_eq!(click.component_id, "delete");
}

#[test]
fn a_row_that_the_document_selects_is_selected_until_the_person_changes_it() {
    let mut document = every_component();
    let Some(Component::List { rows, .. }) = document.components.get_mut(6) else {
        panic!("the fixture holds a list");
    };
    rows[0].selected = true;
    let mut card = CardRuntime::default();
    let frames = CardFrames::new();

    let nodes = frames.pass(&document, &mut card, Owner::Listening, Vec::new());
    let row = nodes
        .iter()
        .find(|(_, node)| node.label() == Some("CX22 - 2 vCPU"))
        .expect("the row is drawn");
    assert_eq!(
        row.1.toggled(),
        Some(egui::accesskit::Toggled::True),
        "the default row draws as selected"
    );
    assert_eq!(card.selected(6), Some("size.cx22"));

    frames.press(&document, &mut card, Owner::Listening, "Connect");
    let click = card.take_click(1_000).expect("a press on a live button is a click");
    assert_eq!(
        click.selected,
        vec!["size.cx22".to_owned()],
        "a click reports the default row until the person changes it"
    );

    frames.press(&document, &mut card, Owner::Listening, "Google Cloud");
    assert_eq!(card.selected(6), Some("gcloud"), "the person's row wins");
    let click = card.take_click(1_000).expect("a press on a row is a click");
    assert_eq!(click.selected, vec!["gcloud".to_owned()]);
}
