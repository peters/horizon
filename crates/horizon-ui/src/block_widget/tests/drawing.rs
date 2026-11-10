//! Every component draws, in both themes and in all three owner states.

use horizon_browser_control::block::{BlockDocument, Button, Component, Selection, Tone};
use horizon_core::CardRuntime;

use crate::theme;

use super::{CardFrames, Owner, card_nodes, card_nodes_in, disabled_state, every_component, one_button};
use crate::block_widget::{AGENT_GONE_LINE, AGENT_GONE_TITLE, CLICKS_REFUSED_LINE, CLICKS_REFUSED_TITLE, EMPTY_CARD};

#[test]
fn every_component_draws_its_text() {
    let nodes = card_nodes(&every_component(), Owner::Listening);

    let drawn = |text: &str| nodes.iter().any(|(label, _)| label == text);
    for text in [
        "build-1",
        "Hetzner · Helsinki",
        "Running",
        "needs your choice",
        "Address",
        "203.0.113.10",
        "Up for",
        "2 h 14 min",
        "Billed from 14:02 UTC.",
        "Step 1 of 3",
        "Provider",
        "CX22 - 2 vCPU",
        "0.42 EUR/h",
        "Google Cloud",
        "gcloud is not installed",
        "Connect",
        "Stop",
        "Delete",
    ] {
        assert!(drawn(text), "{text} must be drawn: {nodes:?}");
    }
}

#[test]
fn every_component_draws_in_both_themes_and_every_owner_state() {
    for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
        for owner in Owner::ALL {
            let nodes = card_nodes_in(resolved, &every_component(), owner);
            assert!(
                nodes.iter().any(|(text, _)| text == "Connect"),
                "the card draws in {resolved:?} with {owner:?}: {nodes:?}"
            );
        }
    }
}

#[test]
fn an_empty_card_says_that_it_is_empty() {
    let nodes = card_nodes(&BlockDocument { components: Vec::new() }, Owner::Listening);

    assert!(
        nodes.iter().any(|(text, _)| text == EMPTY_CARD),
        "the empty card must say it is empty: {nodes:?}"
    );
}

#[test]
fn a_gone_owner_draws_the_status_frame_and_disables_every_control() {
    let nodes = card_nodes(&every_component(), Owner::Gone);

    for text in ["Connect", "Stop", "Delete", "CX22 - 2 vCPU"] {
        assert_eq!(
            disabled_state(&nodes, text),
            Some(true),
            "{text} must be disabled while the agent is away: {nodes:?}"
        );
    }
    assert!(
        nodes.iter().any(|(text, _)| text == AGENT_GONE_TITLE),
        "the card explains why it is dead: {nodes:?}"
    );
    assert!(
        nodes.iter().any(|(text, _)| text == AGENT_GONE_LINE),
        "the card says what to do about it: {nodes:?}"
    );
}

#[test]
fn a_present_owner_leaves_every_control_live() {
    for owner in [Owner::Listening, Owner::Present] {
        let nodes = card_nodes(&every_component(), owner);
        for text in ["Connect", "Delete"] {
            assert_eq!(
                disabled_state(&nodes, text),
                Some(false),
                "{text} must be live with the agent present: {nodes:?}"
            );
        }
        assert!(
            !nodes.iter().any(|(text, _)| text == AGENT_GONE_TITLE),
            "a card with its agent present does not excuse itself: {nodes:?}"
        );
    }
}

#[test]
fn a_refused_click_draws_the_info_frame() {
    let document = one_button();
    let mut card = CardRuntime::default();
    for press in 0..=horizon_core::MAX_PENDING_CLICKS {
        let at = 1_000 + i64::try_from(press).expect("a small click count");
        let _ = card.press("connect", 3, &document, at);
    }
    assert!(card.refused_notice(1_000), "the queue kept the first clicks");

    let nodes: Vec<(String, bool)> = CardFrames::new()
        .pass(&document, &mut card, Owner::Listening, Vec::new())
        .into_iter()
        .filter_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            Some((text.to_owned(), node.is_disabled()))
        })
        .collect();

    assert!(nodes.iter().any(|(text, _)| text == CLICKS_REFUSED_TITLE), "{nodes:?}");
    assert!(nodes.iter().any(|(text, _)| text == CLICKS_REFUSED_LINE), "{nodes:?}");
}

#[test]
fn a_list_that_offers_no_selection_draws_its_rows_as_text() {
    let document = BlockDocument {
        components: vec![Component::List {
            selection: Selection::None,
            rows: vec![horizon_browser_control::block::ListRow {
                id: "row.1".to_string(),
                title: "A row".to_string(),
                subtitle: None,
                tone: Some(Tone::Warn),
            }],
        }],
    };
    let nodes = card_nodes(&document, Owner::Listening);

    assert!(
        nodes.iter().any(|(text, _)| text == "A row"),
        "a row is drawn: {nodes:?}"
    );
}

#[test]
fn a_button_without_a_tone_still_draws() {
    let document = BlockDocument {
        components: vec![Component::Buttons {
            items: vec![Button {
                id: "later".to_string(),
                label: "Later".to_string(),
                tone: Tone::Neutral,
            }],
        }],
    };
    let nodes = card_nodes(&document, Owner::Listening);
    assert!(nodes.iter().any(|(text, _)| text == "Later"), "{nodes:?}");
}
