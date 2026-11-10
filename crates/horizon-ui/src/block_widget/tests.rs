use egui::{Pos2, Rect, Vec2, accesskit};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, ListRow, Selection, Tone};
use horizon_core::{CardRuntime, PanelId};

use super::{AGENT_ABSENT, CardView, EMPTY_CARD, show};
use crate::test_egui::DiscardTextures;

/// Room for every component of the fixture document.
fn card_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 900.0))),
        ..egui::RawInput::default()
    }
}

/// A card body drawn for several passes in one egui context, so a press and a
/// release reach the same widget.
struct CardFrames {
    ctx: egui::Context,
}

impl CardFrames {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self { ctx }
    }

    /// One pass, as an accesskit tree, with the events of that pass.
    fn pass(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        agent_present: bool,
        events: Vec<egui::Event>,
    ) -> Vec<(accesskit::NodeId, accesskit::Node)> {
        let mut input = card_input();
        input.events = events;
        let output = self
            .ctx
            .run_ui(input, |ui| {
                show(
                    ui,
                    PanelId(7),
                    CardView {
                        document,
                        card,
                        revision: 3,
                        agent_present,
                        interactive: true,
                        now_millis: 1_000,
                    },
                );
            })
            .discard_textures();
        let update = output
            .platform_output
            .accesskit_update
            .expect("the card body builds an accesskit tree");
        update.nodes
    }

    /// A press and a release on the center of the widget labelled `label`.
    fn press(&self, document: &BlockDocument, card: &mut CardRuntime, agent_present: bool, label: &str) {
        let position = center_of(self.pass(document, card, agent_present, Vec::new()), label);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = self.pass(document, card, agent_present, vec![egui::Event::PointerMoved(position)]);
        let _ = self.pass(document, card, agent_present, vec![button(true)]);
        let _ = self.pass(document, card, agent_present, vec![button(false)]);
    }
}

/// The screen position of the centre of the widget labelled `label`.
#[allow(clippy::cast_possible_truncation)] // A coordinate of this window fits an f32 exactly.
fn center_of(nodes: Vec<(accesskit::NodeId, accesskit::Node)>, label: &str) -> Pos2 {
    let bounds = nodes
        .into_iter()
        .find(|(_, node)| node.label().is_some_and(|text| text == label))
        .and_then(|(_, node)| node.bounds())
        .unwrap_or_else(|| panic!("{label} has a screen position"));
    Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    )
}

/// The text and disabled state of every widget the card body drew.
fn card_nodes(document: &BlockDocument, agent_present: bool) -> Vec<(String, bool)> {
    CardFrames::new()
        .pass(document, &mut CardRuntime::default(), agent_present, Vec::new())
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

fn disabled_state(nodes: &[(String, bool)], label: &str) -> Option<bool> {
    nodes
        .iter()
        .find(|(text, _)| text == label)
        .map(|(_, disabled)| *disabled)
}

#[test]
fn an_empty_card_says_that_it_is_empty() {
    let nodes = card_nodes(&BlockDocument { components: Vec::new() }, true);

    assert!(
        nodes.iter().any(|(text, _)| text == EMPTY_CARD),
        "the empty card must say it is empty: {nodes:?}"
    );
}

#[test]
fn every_component_draws() {
    let nodes = card_nodes(&every_component(), true);

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
}

#[test]
fn buttons_are_disabled_and_the_card_says_so_without_the_agent() {
    let nodes = card_nodes(&every_component(), false);

    for button in ["Connect", "Delete"] {
        assert_eq!(
            disabled_state(&nodes, button),
            Some(true),
            "{button} must be disabled while the agent is away: {nodes:?}"
        );
    }
    assert!(
        nodes.iter().any(|(text, _)| text == AGENT_ABSENT),
        "the card must say that its agent is away: {nodes:?}"
    );
    assert_eq!(
        disabled_state(&nodes, "CX22 - 2 vCPU"),
        Some(false),
        "a row is drawn as plain text while the agent is away: {nodes:?}"
    );
}

#[test]
fn a_button_is_live_while_the_agent_is_there() {
    let nodes = card_nodes(&every_component(), true);

    for button in ["Connect", "Delete"] {
        assert_eq!(
            disabled_state(&nodes, button),
            Some(false),
            "{button} must be live while the agent is there: {nodes:?}"
        );
    }
    assert!(
        !nodes.iter().any(|(text, _)| text == AGENT_ABSENT),
        "a card with its agent present does not excuse itself: {nodes:?}"
    );
}

#[test]
fn a_press_on_a_button_records_a_click_with_the_revision_the_person_saw() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, true, "Connect");

    let click = card.take_click(1_000).expect("a press on a live button is a click");
    assert_eq!(click.component_id, "connect");
    assert_eq!(click.revision, 3);
    assert!(click.selected.is_empty(), "no row was selected");
}

#[test]
fn a_press_on_a_row_selects_it_and_is_a_click_with_the_selection() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, true, "CX32 - 4 vCPU");

    let click = card.take_click(1_000).expect("a press on a live row is a click");
    assert_eq!(click.component_id, "size.cx32");
    assert_eq!(click.selected, vec!["size.cx32".to_owned()]);
    assert_eq!(card.selected(6), Some("size.cx32"));
}

#[test]
fn a_press_records_nothing_while_the_agent_is_away() {
    let document = every_component();
    let mut card = CardRuntime::default();
    CardFrames::new().press(&document, &mut card, false, "Connect");

    assert!(
        card.take_click(1_000).is_none(),
        "a card without its agent takes no click"
    );
    assert_eq!(card.selected(6), None, "and selects no row");
}
