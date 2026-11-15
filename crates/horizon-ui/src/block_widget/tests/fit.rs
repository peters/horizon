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
use crate::block_widget::metrics::{CARD_MIN_HEIGHT, FOOTER_HEIGHT, MORE_BELOW_HEIGHT};
use crate::block_widget::{CARD_HEIGHT_LIMITS, CardView, fitted_card_height, show};
use crate::test_egui::DiscardTextures;
use crate::theme;

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
        width: None,
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
    let mut opened_link = None;
    let mut pressed = None;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
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
                            host_owned: false,
                            interactive: true,
                            now_millis: 1_000,
                            owner_name: Owner::Listening.name(),
                            owner_session: None,
                            updated_at_millis: 1_790_883_960_000,
                            opened_link: &mut opened_link,
                            pressed: &mut pressed,
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

/// The document of the notice that Horizon draws when Set up finds no agent:
/// a heading with a long second line, the rows of the programs, a line of text
/// and the two buttons of Horizon.
fn notice_document() -> BlockDocument {
    let rows = (0..6)
        .map(|index| KvRow {
            label: format!("Agent {index}"),
            value: "not found".to_string(),
            mono: true,
        })
        .chain(std::iter::once(KvRow {
            label: "Agent integrations".to_string(),
            value: "on".to_string(),
            mono: true,
        }))
        .collect();
    BlockDocument {
        width: None,
        components: vec![
            Component::Heading {
                text: "Set up needs an agent".to_string(),
                subtitle: Some(
                    "An agent in a panel does the work of Set up and shows each step as a card. \
                     Horizon found no agent program on this computer."
                        .to_string(),
                ),
                badge: None,
            },
            Component::Kv { rows },
            Component::Text {
                text: "Install one agent program and sign in to it. Then press Set up again. \
                       Horizon installs nothing for you and stores no login of the agent."
                    .to_string(),
            },
            Component::Buttons {
                items: vec![
                    button("look_again", "Look again", Tone::Primary),
                    button("open_presets", "Open Settings, Presets", Tone::Neutral),
                ],
            },
        ],
    }
}

/// The text shape that drew `text` in a pass of the card that `document` draws.
fn text_shape<'a>(output: &'a egui::FullOutput, text: &str) -> &'a egui::epaint::TextShape {
    let mut found = None;
    for clipped in &output.shapes {
        fn walk<'a>(shape: &'a egui::Shape, text: &str, out: &mut Option<&'a egui::epaint::TextShape>) {
            match shape {
                egui::Shape::Text(shape) if shape.galley.text() == text => *out = Some(shape),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, text, out);
                    }
                }
                _ => {}
            }
        }
        walk(&clipped.shape, text, &mut found);
        if found.is_some() {
            break;
        }
    }
    found.unwrap_or_else(|| panic!("the card drew {text:?}"))
}

/// The text shape that drew the second line of a heading, in a pass of the
/// card that `document` draws.
fn subtitle_shape<'a>(output: &'a egui::FullOutput, subtitle: &str) -> &'a egui::epaint::TextShape {
    text_shape(output, subtitle)
}

/// The heading of the question card that Horizon drew in the live run, and the
/// two buttons of it.
fn question_document(heading: &str) -> BlockDocument {
    BlockDocument {
        width: None,
        components: vec![
            Component::Heading {
                text: heading.to_string(),
                subtitle: None,
                badge: None,
            },
            Component::Kv {
                rows: vec![KvRow {
                    label: "Server target".to_string(),
                    value: "root@204.0.113.56".to_string(),
                    mono: true,
                }],
            },
            Component::Buttons {
                items: vec![
                    button("link", "Link", Tone::Primary),
                    button("not_now", "Not now", Tone::Neutral),
                ],
            },
        ],
    }
}

#[test]
fn the_heading_of_a_card_wraps_instead_of_ending_in_an_ellipsis() {
    // The live card cut this heading at `... the Horizon server o…`: a heading
    // that names its target must show all of it.
    let heading = "Link this computer to the Horizon server on root@204.0.113.56?";
    let document = question_document(heading);
    let mut card = CardRuntime::default();

    let (output, measured) = draw_card(card_panel(560.0), &document, &mut card);

    let drawn = text_shape(&output, heading);
    assert!(
        drawn.galley.rows.len() >= 2,
        "the heading takes the lines that its words need: {:?}",
        drawn.galley.rows.len()
    );
    assert!(!drawn.galley.elided, "no word of the heading is dropped");
    let body = card_body(card_panel(560.0));
    let ink = Rect::from_min_size(drawn.pos, drawn.galley.size());
    assert!(
        ink.max.x <= body.max.x + 0.5,
        "the heading stays inside the card: {ink:?} against {body:?}"
    );

    // The card that the fitted height gives holds the whole heading and its
    // buttons above the footer.
    let height = fitted_card_height(measured, false, CARD_MIN_HEIGHT, CARD_HEIGHT_LIMITS);
    let panel = card_panel(height);
    let (fitted, remeasured) = draw_card(panel, &document, &mut card);
    let room = card_body(panel).height() - FOOTER_HEIGHT;
    assert!(
        remeasured <= room + 0.5,
        "the fitted card holds its content without a scroll area: {remeasured} against {room}"
    );
    let wrapped = text_shape(&fitted, heading);
    assert!(!wrapped.galley.elided, "the fitted card drops no word either");
}

/// The estimate of a card of a heading that needs two lines counts both.
#[test]
fn the_height_estimate_of_a_heading_counts_its_lines() {
    let short = question_document("Link this computer?");
    let long = question_document("Link this computer to the Horizon server on root@204.0.113.56?");
    let short = crate::block_widget::default_card_size(&short, 0.0)[1];
    let long = crate::block_widget::default_card_size(&long, 0.0)[1];
    assert!(
        long > short,
        "a heading of two lines asks for a taller card: {long} against {short}"
    );
}

#[test]
fn the_second_line_of_a_heading_wraps_instead_of_ending_in_an_ellipsis() {
    let document = notice_document();
    let Some(Component::Heading {
        subtitle: Some(subtitle),
        ..
    }) = document.components.first()
    else {
        panic!("the notice starts with a heading and its second line");
    };
    let subtitle = subtitle.clone();
    let mut card = CardRuntime::default();

    let (output, _) = draw_card(card_panel(560.0), &document, &mut card);

    let drawn = subtitle_shape(&output, &subtitle);
    assert!(
        drawn.galley.rows.len() >= 2,
        "the second line takes the lines that its words need: {:?}",
        drawn.galley.rows.len()
    );
    assert!(!drawn.galley.elided, "no word of the second line is dropped");
    let body = card_body(card_panel(560.0));
    let ink = Rect::from_min_size(drawn.pos, drawn.galley.size());
    assert!(
        ink.max.x <= body.max.x + 0.5,
        "the second line stays inside the card: {ink:?} against {body:?}"
    );
}

#[test]
fn a_notice_of_horizon_holds_its_buttons_above_the_footer() {
    let document = notice_document();
    let mut card = CardRuntime::default();
    let estimated = crate::block_widget::default_card_size(&document, 0.0)[1];

    let (_, measured) = draw_card(card_panel(estimated), &document, &mut card);
    let height = fitted_card_height(measured, false, estimated, CARD_HEIGHT_LIMITS);
    let panel = card_panel(height);
    let (output, remeasured) = draw_card(panel, &document, &mut card);
    let body = card_body(panel);
    let room = body.height() - FOOTER_HEIGHT;

    assert!(
        remeasured <= room + 0.5,
        "the fitted card holds its content without a scroll area: {remeasured} against {room}"
    );
    let (label_top, _) = drawn_text(&output, "Look again");
    assert!(
        label_top.y < body.max.y - FOOTER_HEIGHT,
        "the buttons of the notice stand above the footer: {} against {}",
        label_top.y,
        body.max.y - FOOTER_HEIGHT
    );
}

#[test]
fn a_short_document_gets_a_card_that_ends_right_after_its_content() {
    // One heading and one button row: the card that the live run opened at 220
    // points left an empty band above the footer.
    let mut components = BlockDocument {
        width: None,
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

/// The fade of the mark at the lower edge of `body`, as the mesh that painted it.
fn fade_of(output: &egui::FullOutput, body: Rect) -> Option<egui::epaint::Mesh> {
    let bottom = body.max.y - FOOTER_HEIGHT;
    output.shapes.iter().find_map(|clipped| match &clipped.shape {
        Shape::Mesh(mesh) if mesh.vertices.len() == 4 => {
            let top = mesh.vertices.iter().map(|vertex| vertex.pos.y).fold(f32::MAX, f32::min);
            let low = mesh.vertices.iter().map(|vertex| vertex.pos.y).fold(f32::MIN, f32::max);
            ((low - bottom).abs() < 0.01 && (top - (bottom - MORE_BELOW_HEIGHT)).abs() < 0.01).then(|| (**mesh).clone())
        }
        _ => None,
    })
}

#[test]
fn a_body_that_holds_more_fades_into_the_body_colour_and_marks_its_lower_edge() {
    let body = card_body(smallest_card());
    for resolved in [theme::ResolvedTheme::Dark, theme::ResolvedTheme::Light] {
        let _theme = crate::test_egui::with_theme(resolved);
        let mut card = CardRuntime::default();
        let (output, content_height) = draw_card(smallest_card(), &tall_document(), &mut card);
        assert!(
            content_height > body.height() - FOOTER_HEIGHT,
            "the content of the card needs a scroll in {resolved:?}"
        );

        let fade = fade_of(&output, body).unwrap_or_else(|| panic!("the pass paints no fade in {resolved:?}"));
        assert!(
            fade.vertices
                .iter()
                .all(|vertex| vertex.color == theme::PANEL_BG() || vertex.color == theme::alpha(theme::PANEL_BG(), 0)),
            "the fade takes the body colour in {resolved:?}: {:?}",
            fade.vertices.iter().map(|vertex| vertex.color).collect::<Vec<_>>()
        );

        let words = text_shape(&output, "More below");
        let format = &words.galley.job.sections[0].format;
        assert_eq!(
            format.color,
            theme::FG_SOFT(),
            "the words take the soft colour in {resolved:?}"
        );
        assert!(
            (format.font_id.size - 11.0).abs() < f32::EPSILON,
            "the words are 11 point in {resolved:?}"
        );
        assert!(
            words.pos.y > body.max.y - FOOTER_HEIGHT - MORE_BELOW_HEIGHT,
            "the words stand at the lower edge of the body in {resolved:?}: {:?}",
            words.pos
        );

        let nodes = output
            .platform_output
            .accesskit_update
            .expect("the card builds an accessibility tree")
            .nodes;
        assert!(
            nodes
                .iter()
                .any(|(_, node)| node.label() == Some("More below, press to scroll")),
            "a screen reader hears the mark in {resolved:?}"
        );
    }
}

#[test]
fn a_body_that_holds_all_of_its_content_paints_no_mark() {
    let mut card = CardRuntime::default();
    let empty = BlockDocument {
        width: None,
        components: Vec::new(),
    };
    let (output, _) = draw_card(smallest_card(), &empty, &mut card);

    assert!(
        fade_of(&output, card_body(smallest_card())).is_none(),
        "no fade on a card that fits"
    );
    assert!(
        !output
            .shapes
            .iter()
            .any(|clipped| matches!(&clipped.shape, Shape::Text(shape) if shape.galley.text() == "More below")),
        "no mark on a card that fits"
    );
}
