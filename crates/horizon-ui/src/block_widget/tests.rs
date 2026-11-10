use egui::{Pos2, Rect, Vec2};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, ListRow, Selection, Tone};
use horizon_core::PanelId;

use super::{EMPTY_CARD, show};
use crate::test_egui::DiscardTextures;

/// Room for every component of the fixture document.
fn card_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 900.0))),
        ..egui::RawInput::default()
    }
}

/// One headless pass of the card body, with the text and disabled state of
/// every widget it drew.
fn card_nodes(document: &BlockDocument) -> Vec<(String, bool)> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let output = ctx
        .run_ui(card_input(), |ui| show(ui, PanelId(7), document))
        .discard_textures();
    let update = output
        .platform_output
        .accesskit_update
        .expect("the card body builds an accesskit tree");
    update
        .nodes
        .into_iter()
        .filter_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            Some((text.to_owned(), node.is_disabled()))
        })
        .collect()
}

fn every_component() -> BlockDocument {
    BlockDocument {
        components: vec![
            Component::Heading {
                text: "cloud-1".to_string(),
            },
            Component::Badge {
                text: "running".to_string(),
                tone: Tone::Ok,
            },
            Component::Kv {
                rows: vec![
                    KvRow {
                        label: "State".to_string(),
                        value: "running".to_string(),
                    },
                    KvRow {
                        label: "Address".to_string(),
                        value: "203.0.113.10".to_string(),
                    },
                ],
            },
            Component::Text {
                text: "Billed from 14:02 UTC.".to_string(),
            },
            Component::Progress {
                label: "Hour 3 of 24".to_string(),
                percent: 12,
            },
            Component::Divider,
            Component::List {
                selection: Selection::Single,
                rows: vec![
                    ListRow {
                        id: "size.cx22".to_string(),
                        title: "CX22 - 2 vCPU".to_string(),
                        subtitle: Some("0.42 EUR/h".to_string()),
                    },
                    ListRow {
                        id: "size.cx32".to_string(),
                        title: "CX32 - 4 vCPU".to_string(),
                        subtitle: None,
                    },
                ],
            },
            Component::Buttons {
                items: vec![
                    Button {
                        id: "connect".to_string(),
                        label: "Connect".to_string(),
                        tone: Tone::Primary,
                    },
                    Button {
                        id: "delete".to_string(),
                        label: "Delete".to_string(),
                        tone: Tone::Danger,
                    },
                ],
            },
        ],
    }
}

#[test]
fn an_empty_card_says_that_it_is_empty() {
    let nodes = card_nodes(&BlockDocument { components: Vec::new() });

    assert!(
        nodes.iter().any(|(text, _)| text == EMPTY_CARD),
        "the empty card must say it is empty: {nodes:?}"
    );
}

#[test]
fn every_component_draws_and_a_button_stays_disabled() {
    let nodes = card_nodes(&every_component());

    let drawn = |text: &str| nodes.iter().any(|(label, _)| label == text);
    for text in [
        "cloud-1",
        "running",
        "State",
        "203.0.113.10",
        "Billed from 14:02 UTC.",
        "Hour 3 of 24",
        "CX22 - 2 vCPU",
        "0.42 EUR/h",
        "CX32 - 4 vCPU",
        "Connect",
        "Delete",
    ] {
        assert!(drawn(text), "{text} must be drawn: {nodes:?}");
    }
    for button in ["Connect", "Delete"] {
        let disabled = nodes
            .iter()
            .find(|(text, _)| text == button)
            .map(|(_, disabled)| *disabled);
        assert_eq!(disabled, Some(true), "{button} must be disabled: {nodes:?}");
    }
}
