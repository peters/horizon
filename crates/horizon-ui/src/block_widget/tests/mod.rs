//! The card body drawn headless: one egui pass, no window.
//!
//! Every test here draws the real widget into an egui context and reads the
//! accessibility tree afterwards, so a text that was painted is also a text the
//! person can hear. The three owner states and both themes are covered, because
//! the card looks different in each and none of them may panic.

mod clicks;
mod drawing;
mod fit;
mod rules;

use egui::{Pos2, Rect, Vec2, accesskit};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, ListRow, Selection, Tone};
use horizon_core::{CardRuntime, PanelId};

use super::{CardView, show};
use crate::test_egui::DiscardTextures;
use crate::theme;

/// Who owns the card: the three states the chip and the footer show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    /// An agent panel of the owner waits for a click.
    Listening,
    /// An agent panel of the owner is in the workspace but not waiting.
    Present,
    /// No agent panel of the owner is in the workspace.
    Gone,
}

impl Owner {
    /// Whether an agent panel of the owner is in the card's workspace.
    pub(super) const fn present(self) -> bool {
        !matches!(self, Self::Gone)
    }

    /// The display name of the owner, while it is present.
    pub(super) const fn name(self) -> Option<&'static str> {
        match self {
            Self::Listening | Self::Present => Some("Claude"),
            Self::Gone => None,
        }
    }

    /// Every state, for a test that must cover all of them.
    pub(super) const ALL: [Self; 3] = [Self::Listening, Self::Present, Self::Gone];
}

/// Room for every component of the fixture document.
fn card_input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 900.0))),
        ..egui::RawInput::default()
    }
}

/// A card body drawn for several passes in one egui context, so a press and a
/// release reach the same widget.
pub(super) struct CardFrames {
    ctx: egui::Context,
}

impl CardFrames {
    pub(super) fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self { ctx }
    }

    /// One pass, as an accessibility tree, with the events of that pass.
    pub(super) fn pass(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
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
                        agent_present: owner.present(),
                        interactive: true,
                        now_millis: 1_000,
                        owner_name: owner.name(),
                        updated_at_millis: 1_790_883_960_000,
                    },
                );
            })
            .discard_textures();
        let update = output
            .platform_output
            .accesskit_update
            .expect("the card body builds an accessibility tree");
        update.nodes
    }

    /// A press and a release on the center of the widget labelled `label`.
    pub(super) fn press(&self, document: &BlockDocument, card: &mut CardRuntime, owner: Owner, label: &str) {
        let nodes = self.pass(document, card, owner, Vec::new());
        let position = center_of(&nodes, label);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = self.pass(document, card, owner, vec![egui::Event::PointerMoved(position)]);
        let _ = self.pass(document, card, owner, vec![button(true)]);
        let _ = self.pass(document, card, owner, vec![button(false)]);
    }
}

/// The screen position of the centre of the widget labelled `label`.
#[allow(clippy::cast_possible_truncation)] // A coordinate of this window fits an f32 exactly.
fn center_of(nodes: &[(accesskit::NodeId, accesskit::Node)], label: &str) -> Pos2 {
    let bounds = bounds_of(nodes, label).unwrap_or_else(|| panic!("{label} has a screen position"));
    Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    )
}

/// The screen rectangle of the node labelled `label`.
fn bounds_of(nodes: &[(accesskit::NodeId, accesskit::Node)], label: &str) -> Option<accesskit::Rect> {
    nodes
        .iter()
        .find(|(_, node)| node.label().is_some_and(|text| text == label))
        .and_then(|(_, node)| node.bounds())
}

/// The text and disabled state of every widget the card body drew.
fn card_nodes(document: &BlockDocument, owner: Owner) -> Vec<(String, bool)> {
    CardFrames::new()
        .pass(document, &mut CardRuntime::default(), owner, Vec::new())
        .into_iter()
        .filter_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            Some((text.to_owned(), node.is_disabled()))
        })
        .collect()
}

/// The text and disabled state of every widget in `document`, drawn in
/// `resolved`'s palette.
fn card_nodes_in(resolved: theme::ResolvedTheme, document: &BlockDocument, owner: Owner) -> Vec<(String, bool)> {
    let previous = theme::current_theme();
    theme::set_theme(resolved);
    let nodes = card_nodes(document, owner);
    theme::set_theme(previous);
    nodes
}

fn disabled_state(nodes: &[(String, bool)], label: &str) -> Option<bool> {
    nodes
        .iter()
        .find(|(text, _)| text == label)
        .map(|(_, disabled)| *disabled)
}

/// A document that holds every component the card can draw.
pub(super) fn every_component() -> BlockDocument {
    BlockDocument {
        components: vec![
            Component::Heading {
                text: "build-1".to_string(),
                subtitle: Some("Hetzner · Helsinki".to_string()),
                badge: Some(horizon_browser_control::block::Badge {
                    text: "Running".to_string(),
                    tone: Tone::Ok,
                }),
            },
            Component::Badge {
                text: "needs your choice".to_string(),
                tone: Tone::Info,
            },
            Component::Kv {
                rows: vec![
                    KvRow {
                        label: "Address".to_string(),
                        value: "203.0.113.10".to_string(),
                        mono: true,
                    },
                    KvRow {
                        label: "Up for".to_string(),
                        value: "2 h 14 min".to_string(),
                        mono: false,
                    },
                ],
            },
            Component::Text {
                text: "Billed from 14:02 UTC.".to_string(),
            },
            Component::Progress {
                label: "Step 1 of 3".to_string(),
                percent: 33,
                detail: Some("Provider".to_string()),
            },
            Component::Divider,
            Component::List {
                selection: Selection::Single,
                rows: vec![
                    ListRow {
                        id: "size.cx22".to_string(),
                        title: "CX22 - 2 vCPU".to_string(),
                        subtitle: Some("0.42 EUR/h".to_string()),
                        tone: None,
                        selected: false,
                    },
                    ListRow {
                        id: "gcloud".to_string(),
                        title: "Google Cloud".to_string(),
                        subtitle: Some("gcloud is not installed".to_string()),
                        tone: Some(Tone::Warn),
                        selected: false,
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
                        id: "stop".to_string(),
                        label: "Stop".to_string(),
                        tone: Tone::Neutral,
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

/// A document with one button, for tests about clicks and the queue.
pub(super) fn one_button() -> BlockDocument {
    BlockDocument {
        components: vec![Component::Buttons {
            items: vec![Button {
                id: "connect".to_string(),
                label: "Connect".to_string(),
                tone: Tone::Primary,
            }],
        }],
    }
}
