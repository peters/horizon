//! The card body drawn headless: one egui pass, no window.
//!
//! Every test here draws the real widget into an egui context and reads the
//! accessibility tree afterwards, so a text that was painted is also a text the
//! person can hear. The three owner states and both themes are covered, because
//! the card looks different in each and none of them may panic.

mod board;
mod clicks;
mod documents;
mod drawing;
mod fit;
mod lines;
mod link;
mod rules;

use egui::{Pos2, Rect, Vec2, accesskit};
use horizon_browser_control::block::{
    BlockDocument, Button, Component, KvRow, ListLayout, ListRow, Selection, Step, StepState, Tone,
};
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
    with_events(Vec::new())
}

/// Room for every component of the fixture document, with `events` in the pass.
fn with_events(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(720.0, 900.0))),
        events,
        ..egui::RawInput::default()
    }
}

/// One pass of a card body: what it painted, and what its controls asked of the
/// host: the address of a link and the id of a button of a card that Horizon
/// owns.
pub(super) struct Pass {
    pub(super) output: egui::FullOutput,
    opened_link: Option<String>,
    pressed: Option<String>,
}

/// A card body drawn for several passes in one egui context, so a press and a
/// release reach the same widget.
pub(super) struct CardFrames {
    ctx: egui::Context,
    /// The session of a machine that owns the card, when it is one.
    owner_session: Option<(&'static str, &'static str)>,
}

impl CardFrames {
    pub(super) fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Self {
            ctx,
            owner_session: None,
        }
    }

    /// The same frames, with a session named `session` on `machine` as the owner
    /// of the card.
    pub(super) fn on_machine(mut self, machine: &'static str, session: &'static str) -> Self {
        self.owner_session = Some((machine, session));
        self
    }

    /// The same frames, in the fonts of the window, whose widths decide the room
    /// that the footer texts take.
    pub(super) fn in_window_fonts(self) -> Self {
        self.ctx.set_fonts(crate::app::bootstrap::configure_fonts());
        self
    }

    /// One pass, as the shapes it painted.
    pub(super) fn paint(&self, document: &BlockDocument, card: &mut CardRuntime, owner: Owner) -> egui::FullOutput {
        self.run(document, card, owner, card_input()).output
    }

    /// [`Self::paint`] with the card drawn under a canvas zoom, as a card of the
    /// board is drawn.
    pub(super) fn paint_at(
        &self,
        zoom: f32,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
    ) -> egui::FullOutput {
        let mut opened_link = None;
        let mut pressed = None;
        let mut reveal = false;
        let transform = egui::emath::TSTransform::from_scaling(zoom);
        self.ctx
            .run_ui(card_input(), |ui| {
                ui.ctx().set_transform_layer(ui.layer_id(), transform);
                show(
                    ui,
                    PanelId(7),
                    CardView {
                        document,
                        card,
                        revision: 3,
                        agent_present: owner.present(),
                        host_owned: false,
                        interactive: true,
                        now_millis: 1_000,
                        owner_name: owner.name(),
                        owner_session: self.owner_session,
                        updated_at_millis: 1_790_883_960_000,
                        opened_link: &mut opened_link,
                        pressed: &mut pressed,
                        reveal: &mut reveal,
                    },
                );
            })
            .discard_textures()
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
        let output = self.run(document, card, owner, input).output;
        let update = output
            .platform_output
            .accesskit_update
            .expect("the card body builds an accessibility tree");
        update.nodes
    }

    /// One pass in a card `width` wide, as the shapes it painted and the
    /// accessibility tree of that pass.
    pub(super) fn pass_in(
        &self,
        width: f32,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
    ) -> (
        Vec<egui::epaint::ClippedShape>,
        Vec<(accesskit::NodeId, accesskit::Node)>,
    ) {
        let mut input = card_input();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 900.0)));
        let output = self.run(document, card, owner, input).output;
        let update = output
            .platform_output
            .accesskit_update
            .expect("the card body builds an accessibility tree");
        (output.shapes, update.nodes)
    }

    /// One pass in a card `width` wide, as the height that the body recorded
    /// for its content.
    pub(super) fn measured(&self, width: f32, document: &BlockDocument, card: &mut CardRuntime) -> f32 {
        let mut input = card_input();
        input.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 900.0)));
        let _ = self.run(document, card, Owner::Listening, input);
        card.measured_content_height().expect("the pass measures the body")
    }

    fn run(&self, document: &BlockDocument, card: &mut CardRuntime, owner: Owner, input: egui::RawInput) -> Pass {
        self.run_with_interactivity(document, card, owner, input, true)
    }

    /// One pass of the card that Horizon drew itself, as the id of the button
    /// the person pressed in it.
    pub(super) fn press_host(&self, document: &BlockDocument, card: &mut CardRuntime, label: &str) -> Option<String> {
        // The card of Horizon draws no frame of a gone agent, so its buttons
        // sit where the host pass put them, not where an agent's card would.
        let drawn = self.run_as_host(document, card, card_input());
        let nodes = drawn
            .output
            .platform_output
            .accesskit_update
            .expect("a card of Horizon builds an accessibility tree")
            .nodes;
        let position = center_of(&nodes, label);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut passed = self.run_as_host(document, card, with_events(vec![egui::Event::PointerMoved(position)]));
        for pass in [
            self.run_as_host(document, card, with_events(vec![button(true)])),
            self.run_as_host(document, card, with_events(vec![button(false)])),
        ] {
            passed.pressed = passed.pressed.or(pass.pressed);
        }
        passed.pressed
    }

    /// One pass of a card that Horizon owns, with `input` in it.
    pub(super) fn run_as_host(&self, document: &BlockDocument, card: &mut CardRuntime, input: egui::RawInput) -> Pass {
        self.run_with_host(document, card, Owner::Gone, input, true, true)
    }

    /// One pass of a card that a host dialog does (`false`) or does not cover.
    fn run_with_interactivity(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
        input: egui::RawInput,
        interactive: bool,
    ) -> Pass {
        self.run_with_host(document, card, owner, input, interactive, false)
    }

    /// One pass of a card, either of an agent (`host_owned` false) or of
    /// Horizon itself.
    fn run_with_host(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
        input: egui::RawInput,
        interactive: bool,
        host_owned: bool,
    ) -> Pass {
        let mut opened_link = None;
        let mut pressed = None;
        let mut reveal = false;
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
                        host_owned,
                        interactive,
                        now_millis: 1_000,
                        owner_name: owner.name(),
                        owner_session: self.owner_session,
                        updated_at_millis: 1_790_883_960_000,
                        opened_link: &mut opened_link,
                        pressed: &mut pressed,
                        reveal: &mut reveal,
                    },
                );
            })
            .discard_textures();
        Pass {
            output,
            opened_link,
            pressed,
        }
    }

    /// A press and a release on the widget labelled `label`, as the address it
    /// asked Horizon to open.
    pub(super) fn press_link(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
        label: &str,
    ) -> Option<String> {
        self.press_link_with(document, card, owner, label, true)
    }

    /// A press and a release on the widget labelled `label` of a card that a
    /// host dialog covers, as the address it asked Horizon to open.
    pub(super) fn press_link_when_covered(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        label: &str,
    ) -> Option<String> {
        self.press_link_with(document, card, Owner::Listening, label, false)
    }

    fn press_link_with(
        &self,
        document: &BlockDocument,
        card: &mut CardRuntime,
        owner: Owner,
        label: &str,
        interactive: bool,
    ) -> Option<String> {
        let nodes = self.pass(document, card, owner, Vec::new());
        let position = center_of(&nodes, label);
        let button = |pressed: bool| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let moved = self.run_with_interactivity(
            document,
            card,
            owner,
            with_events(vec![egui::Event::PointerMoved(position)]),
            interactive,
        );
        let pressed = self.run_with_interactivity(document, card, owner, with_events(vec![button(true)]), interactive);
        let released =
            self.run_with_interactivity(document, card, owner, with_events(vec![button(false)]), interactive);
        moved.opened_link.or(pressed.opened_link).or(released.opened_link)
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
///
/// A control carries its text as a label, and a text of the card carries it as
/// a value, so a press finds either.
fn bounds_of(nodes: &[(accesskit::NodeId, accesskit::Node)], label: &str) -> Option<accesskit::Rect> {
    nodes
        .iter()
        .find(|(_, node)| node.label().or_else(|| node.value()).is_some_and(|text| text == label))
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
    let _theme = crate::test_egui::with_theme(resolved);
    card_nodes(document, owner)
}

/// Every text of a card that draws `document` with a runtime state of its own,
/// in both themes.
fn texts_of_card_with(document: &BlockDocument, card: &mut CardRuntime, owner: Owner) -> Vec<String> {
    let mut texts = Vec::new();
    for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
        let _theme = crate::test_egui::with_theme(resolved);
        let mut card = card.clone();
        texts.extend(
            card_nodes_with(document, &mut card, owner)
                .into_iter()
                .map(|(text, _)| text),
        );
    }
    texts
}

/// The text and disabled state of every widget that `card` draws.
fn card_nodes_with(document: &BlockDocument, card: &mut CardRuntime, owner: Owner) -> Vec<(String, bool)> {
    CardFrames::new()
        .pass(document, card, owner, Vec::new())
        .into_iter()
        .filter_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            Some((text.to_owned(), node.is_disabled()))
        })
        .collect()
}

fn disabled_state(nodes: &[(String, bool)], label: &str) -> Option<bool> {
    nodes
        .iter()
        .find(|(text, _)| text == label)
        .map(|(_, disabled)| *disabled)
}

/// A document that holds every component the card can draw.
pub(super) fn every_component() -> BlockDocument {
    let mut components = vec![
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
                    value: Some("0.006 EUR/h".to_string()),
                    badge: None,
                    disabled: false,
                },
                ListRow {
                    id: "gcloud".to_string(),
                    title: "Google Cloud".to_string(),
                    subtitle: Some("gcloud is not installed".to_string()),
                    tone: Some(Tone::Warn),
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                },
                ListRow {
                    id: "aws".to_string(),
                    title: "AWS".to_string(),
                    subtitle: Some("No profile found. Run aws configure sso".to_string()),
                    tone: None,
                    selected: false,
                    value: None,
                    badge: Some(horizon_browser_control::block::Badge {
                        text: "Ready".to_string(),
                        tone: Tone::Ok,
                    }),
                    disabled: true,
                },
            ],
            layout: ListLayout::Rows,
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
    ];
    components.extend(pills_and_steps());
    BlockDocument {
        width: None,
        finished: false,
        components,
    }
}

/// The second half of a card: a caption, a choice drawn as pills and the steps
/// of longer work.
fn pills_and_steps() -> Vec<Component> {
    vec![
        Component::Caption {
            text: "Place".to_string(),
        },
        Component::List {
            selection: Selection::Single,
            rows: vec![
                ListRow {
                    id: "place.helsinki".to_string(),
                    title: "Helsinki".to_string(),
                    subtitle: None,
                    tone: None,
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                },
                ListRow {
                    id: "place.falkenstein".to_string(),
                    title: "Falkenstein".to_string(),
                    subtitle: None,
                    tone: None,
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                },
            ],
            layout: ListLayout::Pills,
        },
        Component::Steps {
            steps: vec![
                Step {
                    label: "Create the machine at the provider".to_string(),
                    state: StepState::Done,
                    at: Some("11 s".to_string()),
                },
                Step {
                    label: "Wait for SSH with the machine key".to_string(),
                    state: StepState::Now,
                    at: Some("38 s".to_string()),
                },
                Step {
                    label: "Add the machine to Machines".to_string(),
                    state: StepState::Waiting,
                    at: None,
                },
            ],
        },
    ]
}

/// A document with the steps of a longer piece of work and nothing else.
pub(super) fn steps_document() -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Steps {
            steps: vec![
                Step {
                    label: "Create the machine at the provider".to_string(),
                    state: StepState::Done,
                    at: Some("11 s".to_string()),
                },
                Step {
                    label: "Add the machine to Machines".to_string(),
                    state: StepState::Waiting,
                    at: None,
                },
            ],
        }],
    }
}

/// A document with one list of pills and nothing else.
pub(super) fn pills_document() -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::List {
            selection: Selection::Single,
            rows: vec![
                ListRow {
                    id: "place.helsinki".to_string(),
                    title: "Helsinki".to_string(),
                    subtitle: None,
                    tone: None,
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                },
                ListRow {
                    id: "place.falkenstein".to_string(),
                    title: "Falkenstein".to_string(),
                    subtitle: None,
                    tone: None,
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                },
            ],
            layout: ListLayout::Pills,
        }],
    }
}

/// A document with one button, for tests about clicks and the queue.
pub(super) fn one_button() -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Buttons {
            items: vec![Button {
                id: "connect".to_string(),
                label: "Connect".to_string(),
                tone: Tone::Primary,
            }],
        }],
    }
}
