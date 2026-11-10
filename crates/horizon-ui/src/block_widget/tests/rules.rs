//! The rules the host applies on top of a document: which button is primary,
//! where a destructive button sits, what the footer and the chip say, and how
//! large a new card opens.

use horizon_browser_control::block::{BlockDocument, Button, Component, Tone};
use horizon_core::{CardRuntime, PanelKind};

use super::{CardFrames, Owner, bounds_of, every_component};
use crate::block_widget::chip::{CardChip, ChipState};
use crate::block_widget::{buttons, default_card_size, footer, primary_button};

/// A card of one `buttons` component with the tones in `items`.
fn buttons(items: Vec<Button>) -> BlockDocument {
    BlockDocument {
        components: vec![Component::Buttons { items }],
    }
}

fn button(id: &str, label: &str, tone: Tone) -> Button {
    Button {
        id: id.to_string(),
        label: label.to_string(),
        tone,
    }
}

#[test]
fn only_the_first_primary_button_of_a_card_is_the_primary_one() {
    let document = BlockDocument {
        components: vec![
            Component::Divider,
            Component::Buttons {
                items: vec![
                    button("stop", "Stop", Tone::Neutral),
                    button("connect", "Connect", Tone::Primary),
                ],
            },
            Component::Buttons {
                items: vec![
                    button("again", "Again", Tone::Primary),
                    button("delete", "Delete", Tone::Danger),
                ],
            },
        ],
    };

    assert_eq!(
        primary_button(&document),
        Some((1, 1)),
        "the first primary button wins, in document order"
    );
}

#[test]
fn a_card_without_a_primary_button_has_none() {
    let document = buttons(vec![button("stop", "Stop", Tone::Neutral)]);
    assert_eq!(primary_button(&document), None);
}

#[test]
fn a_later_primary_button_is_drawn_neutral() {
    assert_eq!(buttons::drawn_tone(Tone::Primary, false), Tone::Neutral);
    assert_eq!(buttons::drawn_tone(Tone::Primary, true), Tone::Primary);
    assert_eq!(buttons::drawn_tone(Tone::Danger, false), Tone::Danger);
}

#[test]
fn a_dangerous_button_goes_to_the_right_edge() {
    let document = buttons(vec![
        button("connect", "Connect", Tone::Primary),
        button("delete", "Delete", Tone::Danger),
        button("stop", "Stop", Tone::Neutral),
    ]);
    let nodes = CardFrames::new().pass(&document, &mut CardRuntime::default(), Owner::Listening, Vec::new());
    let connect = bounds_of(&nodes, "Connect").expect("Connect is drawn");
    let stop = bounds_of(&nodes, "Stop").expect("Stop is drawn");
    let delete = bounds_of(&nodes, "Delete").expect("Delete is drawn");

    assert!(
        connect.x1 <= stop.x0,
        "the ordinary buttons keep document order: {connect:?} then {stop:?}"
    );
    assert!(
        delete.x0 >= stop.x1,
        "the destructive button sits at the right edge: {delete:?} after {stop:?}"
    );
}

#[test]
fn the_footer_names_the_owner_or_says_that_an_agent_drew_the_card() {
    assert_eq!(
        footer::drawn_by(Some("Claude")),
        ("Card drawn by the agent", Some("Claude"))
    );
    assert_eq!(footer::drawn_by(None), ("Card drawn by an agent", None));
}

#[test]
fn the_updated_label_is_formatted_once_per_document() {
    let mut card = CardRuntime::default();
    let first = card.updated_label(3, 1_790_883_960_000).to_owned();
    assert!(first.starts_with("updated "), "{first}");

    let again = card.updated_label(3, 1_790_883_960_000).to_owned();
    assert_eq!(again, first, "an unchanged document reuses its label");

    let next = card.updated_label(4, 1_790_883_960_000).to_owned();
    assert_eq!(next, first, "a new revision of the same minute reads the same");

    let unknown = CardRuntime::default().updated_label(1, 0).to_owned();
    assert_eq!(unknown, "", "a card that records no time shows no clock");
}

#[test]
fn the_chip_names_the_agent_and_says_when_it_listens() {
    let listening = CardChip::for_panel(PanelKind::Claude, true, true);
    assert_eq!(listening.state, ChipState::Listening);
    assert_eq!(listening.spoken(), (" listens", "Claude"));

    let present = CardChip::for_panel(PanelKind::Claude, false, true);
    assert_eq!(present.state, ChipState::Present);
    assert_eq!(present.spoken(), ("", "Claude"));

    let gone = CardChip::for_panel(PanelKind::Claude, false, false);
    assert_eq!(gone.state, ChipState::Gone);
    assert_eq!(gone.spoken(), ("", "No agent"), "a gone owner is not named");
}

#[test]
fn a_new_card_is_four_hundred_and_twenty_wide_and_as_tall_as_its_content() {
    let empty = BlockDocument { components: Vec::new() };
    assert_eq!(
        default_card_size(&empty),
        [420.0, 220.0],
        "an empty card keeps the floor"
    );

    let small = BlockDocument {
        components: vec![Component::Kv {
            rows: (0..6)
                .map(|index| horizon_browser_control::block::KvRow {
                    label: format!("Label {index}"),
                    value: "value".to_string(),
                    mono: false,
                })
                .collect(),
        }],
    };
    let [width, height] = default_card_size(&small);
    assert!((width - 420.0).abs() < f32::EPSILON);
    assert!(height > 220.0 && height < 720.0, "{height} fits the content");

    let huge = BlockDocument {
        components: (0..16)
            .map(|_| Component::Kv {
                rows: (0..24)
                    .map(|index| horizon_browser_control::block::KvRow {
                        label: format!("Label {index}"),
                        value: "value".to_string(),
                        mono: false,
                    })
                    .collect(),
            })
            .collect(),
    };
    let [_, height] = default_card_size(&huge);
    assert!(
        (height - 720.0).abs() < f32::EPSILON,
        "a long card stops at the ceiling"
    );
}

#[test]
fn the_card_size_of_a_document_does_not_depend_on_its_owner() {
    let document = every_component();
    let first = default_card_size(&document);
    let second = default_card_size(&document);
    assert_eq!(first, second);
    assert!(first[1] >= 220.0 && first[1] <= 720.0, "{first:?}");
}
