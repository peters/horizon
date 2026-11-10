//! A card that is shorter than its content scrolls its body above the footer.
//!
//! The body records the height its content needs, which is what the host fits
//! the card to. Until then, or when the person resized the card shorter, the
//! content is clipped at the footer: no part of it is drawn under the strip.

use egui::epaint::Shape;
use egui::{Pos2, Rect, Vec2};
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, Tone};
use horizon_core::{CardRuntime, PanelId};

use super::{Owner, one_button};
use crate::app::{PANEL_PADDING, PANEL_TITLEBAR_HEIGHT};
use crate::block_widget::metrics::{CARD_MIN_HEIGHT, FOOTER_HEIGHT};
use crate::block_widget::{CARD_HEIGHT_LIMITS, CardView, fitted_card_height, show};
use crate::test_egui::DiscardTextures;

/// A card panel of the smallest card size, at a known place on the screen.
fn smallest_card() -> Rect {
    Rect::from_min_size(Pos2::new(40.0, 60.0), Vec2::new(420.0, CARD_MIN_HEIGHT))
}

/// The same card panel, `height` tall.
fn card_panel(height: f32) -> Rect {
    Rect::from_min_size(smallest_card().min, Vec2::new(420.0, height))
}

/// The body of a panel, as a card panel draws it: inside the titlebar and the
/// panel padding.
fn card_body(panel: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(
            panel.min.x + PANEL_PADDING,
            panel.min.y + PANEL_TITLEBAR_HEIGHT + PANEL_PADDING,
        ),
        Pos2::new(panel.max.x - PANEL_PADDING, panel.max.y - PANEL_PADDING),
    )
}

fn button(id: &str, label: &str, tone: Tone) -> Button {
    Button {
        id: id.to_string(),
        label: label.to_string(),
        tone,
    }
}

/// A document that needs more room than the smallest card has.
fn tall_document() -> BlockDocument {
    BlockDocument {
        components: vec![
            Component::Kv {
                rows: (0..12)
                    .map(|index| KvRow {
                        label: format!("Label {index}"),
                        value: "value".to_string(),
                        mono: false,
                    })
                    .collect(),
            },
            Component::Buttons {
                items: vec![
                    button("stop", "Stop", Tone::Neutral),
                    button("delete", "Delete", Tone::Danger),
                ],
            },
        ],
    }
}

/// Where the body drew `text`, and the rectangle it was clipped to.
fn drawn_text(output: &egui::FullOutput, text: &str) -> (Pos2, Rect) {
    output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text() == text => Some((shape.pos, clipped.clip_rect)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the card drew {text}"))
}

/// Draws `document` into `panel` and returns the pass and the body height that
/// the card recorded.
fn draw_card(panel: Rect, document: &BlockDocument, card: &mut CardRuntime) -> (egui::FullOutput, f32) {
    let body = card_body(panel);
    let ctx = egui::Context::default();
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))),
                ..egui::RawInput::default()
            },
            |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    show(
                        ui,
                        PanelId(7),
                        CardView {
                            document,
                            card,
                            revision: 3,
                            agent_present: Owner::Listening.present(),
                            interactive: true,
                            now_millis: 1_000,
                            owner_name: Owner::Listening.name(),
                            updated_at_millis: 1_790_883_960_000,
                        },
                    );
                });
            },
        )
        .discard_textures();
    (
        output,
        card.measured_content_height().expect("the pass measures the body"),
    )
}

#[test]
fn a_tall_document_scrolls_in_the_body_and_never_reaches_the_footer() {
    let document = tall_document();
    let mut card = CardRuntime::default();

    let (output, content_height) = draw_card(smallest_card(), &document, &mut card);

    let body = card_body(smallest_card());
    let footer_top = body.max.y - FOOTER_HEIGHT;
    assert!(
        content_height > body.height() - FOOTER_HEIGHT,
        "the content needs a scroll area: {content_height} against {}",
        body.height() - FOOTER_HEIGHT
    );

    let (label_top, clip) = drawn_text(&output, "Delete");
    assert!(
        label_top.y >= footer_top,
        "the last component is laid out below the footer: {} against {footer_top}",
        label_top.y
    );
    assert!(
        clip.max.y <= footer_top + 0.5,
        "the last component is clipped at the footer, not drawn under it: {clip:?}"
    );
}

#[test]
fn a_short_document_gets_a_card_that_ends_right_after_its_content() {
    // One heading and one button row: the card that the live run opened at 220
    // points left an empty band above the footer.
    let mut components = BlockDocument {
        components: vec![Component::Heading {
            text: "home-mac".to_string(),
            subtitle: None,
            badge: None,
        }],
    };
    components.components.extend(one_button().components);
    let mut card = CardRuntime::default();

    let (_, content_height) = draw_card(smallest_card(), &components, &mut card);
    let height = fitted_card_height(content_height, false, CARD_MIN_HEIGHT, CARD_HEIGHT_LIMITS);

    assert!(
        height > CARD_MIN_HEIGHT,
        "the content decides the height above the floor: {height}"
    );
    assert!(
        height < 220.0,
        "a small card no longer keeps an empty band above its footer: {height}"
    );

    let panel = card_panel(height);
    let (output, measured) = draw_card(panel, &components, &mut card);
    let body = card_body(panel);
    assert!(
        measured <= body.height() - FOOTER_HEIGHT + 0.5,
        "the fitted card holds its content without a scroll area: {measured} against {}",
        body.height() - FOOTER_HEIGHT
    );
    let (label_top, _) = drawn_text(&output, "Connect");
    assert!(
        label_top.y < body.max.y - FOOTER_HEIGHT,
        "the button sits above the footer"
    );
}
