//! Every component draws, in both themes and in all three owner states.

use egui::Color32;
use egui::epaint::Shape;
use horizon_browser_control::block::{BlockDocument, Button, Component, Selection, Tone};
use horizon_core::{CardFrames as HostFrames, CardRuntime};

use crate::theme;

use super::{CardFrames, Owner, card_nodes, card_nodes_in, disabled_state, every_component, one_button};
use crate::block_widget::{AGENT_GONE_LINE, AGENT_GONE_TITLE, CLICKS_REFUSED_LINE, CLICKS_REFUSED_TITLE, EMPTY_CARD};

/// Whether the pass painted a rectangle outline of `width` in `color`.
fn painted_outline(output: &egui::FullOutput, width: f32, color: Color32) -> bool {
    output.shapes.iter().any(|clipped| match &clipped.shape {
        Shape::Rect(rect) => (rect.stroke.width - width).abs() < 0.01 && rect.stroke.color == color,
        _ => false,
    })
}

/// Whether the pass painted a filled circle in `color`.
fn painted_dot(output: &egui::FullOutput, color: Color32) -> bool {
    output.shapes.iter().any(|clipped| match &clipped.shape {
        Shape::Circle(circle) => circle.fill == color && circle.fill.a() > 0,
        _ => false,
    })
}

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
fn a_gone_owner_keeps_the_selected_row_in_a_dimmed_form() {
    let mut document = every_component();
    let Some(Component::List { rows, .. }) = document.components.get_mut(6) else {
        panic!("the fixture holds a list");
    };
    rows[1].selected = true;
    let mut card = CardRuntime::default();
    let frames = CardFrames::new();

    let live = frames.paint(&document, &mut card, Owner::Listening);
    assert_eq!(card.selected(6), Some("gcloud"), "the document selects a row");
    assert!(
        painted_outline(&live, 2.0, theme::ACCENT()),
        "a live selected row keeps the accent outline"
    );
    assert!(
        painted_dot(&live, theme::ACCENT()),
        "a live selected row keeps the filled accent mark"
    );

    // The owner leaves: the choice stays on screen, dimmed instead of live.
    let dimmed = theme::alpha(theme::ACCENT(), 120);
    let gone = frames.paint(&document, &mut card, Owner::Gone);

    assert_eq!(card.selected(6), Some("gcloud"), "the choice outlives the owner");
    assert_eq!(
        card.frames(),
        HostFrames {
            agent_gone: true,
            clicks_refused: false,
        },
        "the pass tells the host which frames the body needs room for"
    );
    assert!(
        painted_outline(&gone, 2.0, dimmed),
        "a disabled selected row keeps a dimmed outline"
    );
    assert!(
        painted_dot(&gone, dimmed),
        "a disabled selected row keeps a dimmed radio mark"
    );
    assert!(
        !painted_outline(&gone, 2.0, theme::ACCENT()),
        "no row claims the live accent while the agent is away"
    );

    let nodes = frames.pass(&document, &mut card, Owner::Gone, Vec::new());
    let row = nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Google Cloud"))
        .expect("the row is drawn");
    assert_eq!(
        row.1.toggled(),
        Some(egui::accesskit::Toggled::True),
        "the row reports its selection to a screen reader"
    );
    assert!(row.1.is_disabled(), "the row takes no press without its agent");
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
                selected: false,
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
