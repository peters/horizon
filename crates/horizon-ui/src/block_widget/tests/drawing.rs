//! Every component draws, in both themes and in all three owner states.

use egui::Color32;
use egui::epaint::Shape;
use horizon_browser_control::block::{BlockDocument, Button, Component, ListLayout, Selection, Tone};
use horizon_core::{CardFrames as HostFrames, CardRuntime};

use crate::block_widget::metrics::{BODY_PAD_SIDE, KV_PAD_SIDE, KV_ROW_HEIGHT, KV_ROW_PAD_Y};
use crate::theme;
use crate::typeface::Weight;

use super::{CardFrames, Owner, card_nodes, card_nodes_in, disabled_state, every_component, one_button};
use crate::block_widget::{
    AGENT_GONE_LINE, AGENT_GONE_TITLE, CLICKS_REFUSED_LINE, CLICKS_REFUSED_TITLE, EMPTY_CARD, default_card_size,
};

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

/// The `wght` coordinate of every text of a pass whose words are `text`.
fn painted_weights(output: &egui::FullOutput, text: &str) -> Vec<Option<f32>> {
    fn walk(shape: &Shape, text: &str, out: &mut Vec<Option<f32>>) {
        match shape {
            Shape::Text(shape) if shape.galley.text() == text => out.push(
                shape
                    .galley
                    .job
                    .sections
                    .first()
                    .and_then(|section| section.format.coords.as_ref().first().map(|(_, value)| *value)),
            ),
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, text, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, text, &mut out);
    }
    out
}

/// The colour of every text of a pass whose words are `text`, in paint order.
///
/// A galley paints the colour of its sections; the fallback colour that the
/// painter is handed only fills a section that names no colour.
fn painted_colours(output: &egui::FullOutput, text: &str) -> Vec<Color32> {
    fn walk(shape: &Shape, text: &str, out: &mut Vec<Color32>) {
        match shape {
            Shape::Text(shape) if shape.galley.text() == text => {
                out.extend(shape.galley.job.sections.iter().map(|section| section.format.color));
            }
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, text, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, text, &mut out);
    }
    out
}

/// The weights of the approved card mockups reach the glyphs that the card
/// paints: the heading is heavy, a caption is the heaviest, and the row title
/// and the primary button label stand above the body text.
#[test]
fn the_weights_of_the_mockups_reach_the_painted_card() {
    let frames = CardFrames::new();
    let mut card = CardRuntime::default();
    let output = frames.paint(&every_component(), &mut card, Owner::Listening);

    assert_eq!(painted_weights(&output, "build-1"), vec![Some(Weight::Bold.wght())]);
    assert_eq!(
        painted_weights(&output, "Hetzner · Helsinki"),
        vec![Some(Weight::Regular.wght())],
        "the second line of a heading stays regular"
    );
    assert_eq!(painted_weights(&output, "PLACE"), vec![Some(Weight::Black.wght())]);
    assert_eq!(
        painted_weights(&output, "CX22 - 2 vCPU"),
        vec![Some(Weight::Semibold.wght())],
        "a row title stands above the body text"
    );
    assert_eq!(painted_weights(&output, "Connect"), vec![Some(Weight::Semibold.wght())]);
    assert_eq!(painted_weights(&output, "Stop"), vec![Some(Weight::Regular.wght())]);
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
        "0.006 EUR/h",
        "Ready",
        "Google Cloud",
        "gcloud is not installed",
        "AWS",
        "No profile found. Run aws configure sso",
        "Place",
        "Helsinki",
        "Falkenstein",
        "Create the machine at the provider",
        "11 s",
        "Wait for SSH with the machine key",
        "Add the machine to Machines",
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
    let nodes = card_nodes(
        &BlockDocument {
            width: None,
            finished: false,
            components: Vec::new(),
        },
        Owner::Listening,
    );

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
    let _theme = crate::test_egui::with_theme(theme::current_theme());
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
            link_refused: false,
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
        width: None,
        finished: false,
        components: vec![Component::List {
            selection: Selection::None,
            rows: vec![horizon_browser_control::block::ListRow {
                id: "row.1".to_string(),
                title: "A row".to_string(),
                subtitle: None,
                tone: Some(Tone::Warn),
                selected: false,
                value: None,
                badge: None,
                disabled: false,
            }],
            layout: ListLayout::Rows,
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
        width: None,
        finished: false,
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

/// The pass of a card `width` wide that holds one heading with a badge, drawn
/// while the owner of the card is away, so the badge carries its prefix.
fn narrow_heading(
    width: f32,
    subtitle: Option<&str>,
    badge: &str,
) -> (Vec<egui::epaint::ClippedShape>, Vec<(String, egui::accesskit::Rect)>) {
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Heading {
            text: "home-mac".to_string(),
            subtitle: subtitle.map(str::to_owned),
            badge: Some(horizon_browser_control::block::Badge {
                text: badge.to_string(),
                tone: Tone::Warn,
            }),
        }],
    };
    let (output, nodes) = CardFrames::new().pass_in(width, &document, &mut CardRuntime::default(), Owner::Gone);
    let spoken = nodes
        .iter()
        .filter_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            Some((text.to_owned(), node.bounds()?))
        })
        .collect();
    (output, spoken)
}

/// Where the pass put the text `text`, or `None` when it drew no such text.
fn spoken_at(spoken: &[(String, egui::accesskit::Rect)], text: &str) -> Option<egui::accesskit::Rect> {
    spoken
        .iter()
        .find_map(|(label, bounds)| (label == text).then_some(*bounds))
}

/// Whether the pass elided the heading text instead of drawing all of it.
fn heading_is_elided(output: &[egui::epaint::ClippedShape]) -> bool {
    output
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text() == "home-mac" => Some(shape.galley.elided),
            _ => None,
        })
        .expect("the card drew its heading")
}

#[test]
fn a_narrow_card_draws_the_whole_heading_and_moves_its_badge_under_the_line() {
    let (output, spoken) = narrow_heading(280.0, Some("Hetzner · Helsinki"), "Needs your choice");

    assert!(
        !heading_is_elided(&output),
        "the heading keeps its text on a narrow card: {spoken:?}"
    );
    let badge = spoken_at(&spoken, "Last seen: Needs your choice").expect("the badge is drawn with its prefix");
    let subtitle = spoken_at(&spoken, "Hetzner · Helsinki").expect("the second line is drawn");

    assert!(
        badge.y0 >= subtitle.y1,
        "the badge takes a row of its own under the second line: {badge:?} against {subtitle:?}"
    );
    assert!(
        (badge.x0 - f64::from(BODY_PAD_SIDE)).abs() < 0.5,
        "the badge starts at the left edge of the heading: {badge:?}"
    );
}

#[test]
fn a_wide_card_keeps_the_badge_on_the_heading_row() {
    let (output, spoken) = narrow_heading(420.0, Some("Hetzner · Helsinki"), "Needs your choice");

    assert!(!heading_is_elided(&output), "the heading keeps its text: {spoken:?}");
    let badge = spoken_at(&spoken, "Last seen: Needs your choice").expect("the badge is drawn with its prefix");
    let subtitle = spoken_at(&spoken, "Hetzner · Helsinki").expect("the second line is drawn");

    assert!(
        badge.y0 < subtitle.y0,
        "the badge shares the heading row: {badge:?} against {subtitle:?}"
    );
    let row_right = 420.0 - f64::from(BODY_PAD_SIDE);
    assert!(
        (badge.x1 - row_right).abs() < 0.5,
        "the badge ends at the right edge of the heading row: {badge:?}"
    );
}

#[test]
fn a_row_that_cannot_be_chosen_tells_a_reader_that_it_takes_no_press() {
    let nodes = card_nodes(&every_component(), Owner::Listening);
    assert_eq!(
        disabled_state(&nodes, "AWS"),
        Some(true),
        "a row the document disables reads as disabled"
    );
    assert_eq!(
        disabled_state(&nodes, "CX22 - 2 vCPU"),
        Some(false),
        "a row of a choice reads as live"
    );
}

#[test]
fn a_step_reports_its_own_state_to_a_reader_as_its_line() {
    let nodes = card_nodes(&every_component(), Owner::Listening);
    let lines: Vec<&str> = nodes.iter().map(|(text, _)| text.as_str()).collect();
    assert!(
        lines.contains(&"Wait for SSH with the machine key"),
        "the line of a step is a node of its own: {lines:?}"
    );
    assert!(
        lines.contains(&"38 s"),
        "the time of a step is a node of its own too: {lines:?}"
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|(text, _)| text == "Create the machine at the provider")
            .count(),
        1,
        "a step is one node"
    );
}

#[test]
fn a_failed_step_draws_the_cross_of_a_failed_item_in_the_danger_colour() {
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Steps {
            steps: vec![
                horizon_browser_control::block::Step {
                    label: "Create the machine at the provider".to_string(),
                    state: horizon_browser_control::block::StepState::Done,
                    at: Some("11 s".to_string()),
                },
                horizon_browser_control::block::Step {
                    label: "Wait for SSH with the machine key".to_string(),
                    state: horizon_browser_control::block::StepState::Failed,
                    at: Some("3 m".to_string()),
                },
            ],
        }],
    };
    let output = CardFrames::new().paint(&document, &mut CardRuntime::default(), Owner::Listening);
    assert!(
        painted_dot(&output, theme::PALETTE_RED()),
        "the mark of a failed step is the red disc of a failed board item"
    );
    assert_eq!(
        painted_weights(&output, "Wait for SSH with the machine key"),
        vec![Some(400.0)],
        "the label of a failed step keeps the weight of a line"
    );
    assert_eq!(
        painted_colours(&output, "3 m"),
        vec![theme::PALETTE_RED()],
        "the time of a failed step carries the danger with its mark"
    );
    assert_eq!(
        painted_colours(&output, "11 s"),
        vec![theme::FG_DIM()],
        "the time of a step that finished stays the dim colour of a time"
    );
}

#[test]
fn a_card_that_horizon_owns_says_so_and_shows_no_frame_of_a_gone_agent() {
    use crate::block_widget::footer::HOST_DRAWN;

    let document = one_button();
    let mut card = CardRuntime::default();
    let frames = CardFrames::new();
    let nodes = card_nodes(&document, Owner::Listening);
    assert!(nodes.iter().any(|(text, _)| text == "Connect"), "{nodes:?}");

    let output = frames.run_as_host(&document, &mut card, super::card_input());
    assert_eq!(
        card.frames(),
        HostFrames::default(),
        "the notice of Horizon needs no frame of a gone agent"
    );
    let texts: Vec<String> = output
        .output
        .platform_output
        .accesskit_update
        .expect("an accessibility tree")
        .nodes
        .into_iter()
        .filter_map(|(_, node)| node.label().or_else(|| node.value()).map(str::to_owned))
        .collect();
    assert!(
        texts.iter().any(|text| text == HOST_DRAWN),
        "the footer says that Horizon drew this card: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == AGENT_GONE_TITLE),
        "a card of Horizon's own claims no gone agent: {texts:?}"
    );
}

/// The document of a card that holds one `kv` of `rows`.
fn kv_document(rows: &[(&str, &str)]) -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::Kv {
            rows: rows
                .iter()
                .map(|(label, value)| horizon_browser_control::block::KvRow {
                    label: (*label).to_string(),
                    value: (*value).to_string(),
                    mono: false,
                })
                .collect(),
        }],
    }
}

/// The pass of a card 420 points wide that holds one `kv` of `rows`.
fn kv_card(rows: &[(&str, &str)]) -> Vec<egui::epaint::ClippedShape> {
    let document = kv_document(rows);
    let (output, _) = CardFrames::new().pass_in(420.0, &document, &mut CardRuntime::default(), Owner::Listening);
    output
}

/// The text shape that drew `text` in a pass.
fn text_of<'a>(output: &'a [egui::epaint::ClippedShape], text: &str) -> &'a egui::epaint::TextShape {
    output
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(shape) if shape.galley.text() == text => Some(shape),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the card drew {text}"))
}

/// The right edge of the values of a card: the card width less the padding of
/// the body and of the inset box.
fn value_right() -> f32 {
    420.0 - f32::from(BODY_PAD_SIDE) - KV_PAD_SIDE
}

#[test]
fn a_long_value_keeps_every_line_and_starts_at_its_own_column() {
    let cost = "0.012 EUR / hour, including the IPv4 address of the machine and its traffic";
    let output = kv_card(&[("Cost", cost), ("Session", "helsinki-1")]);

    let value = text_of(&output, cost);
    assert!(!value.galley.elided, "the value is not cut: {value:?}");
    assert!(
        value.galley.rows.len() >= 2,
        "the value wraps to the lines it needs: {:?}",
        value.galley.rows.len()
    );
    let label = text_of(&output, "Cost");
    assert!(
        value.pos.x > label.pos.x,
        "the value starts in its own column, to the right of the label"
    );
    assert!(
        value.pos.x + value.galley.size().x <= value_right() + 0.5,
        "the value ends at or before the right edge of the row: {} against {}",
        value.pos.x + value.galley.size().x,
        value_right()
    );
    // The row grew with its value, so the pair under it starts below it.
    let next = text_of(&output, "helsinki-1");
    let row_bottom = label.pos.y - KV_ROW_PAD_Y + KV_ROW_HEIGHT;
    assert!(
        next.pos.y > row_bottom,
        "the next pair starts below the pair that the long value made taller: {} against {row_bottom}",
        next.pos.y
    );
}

#[test]
fn a_short_value_keeps_the_right_edge_of_its_row() {
    let output = kv_card(&[("Cost", "0.012 EUR / hour")]);

    let value = text_of(&output, "0.012 EUR / hour");
    assert_eq!(value.galley.rows.len(), 1, "one line stays one line");
    assert!(
        (value.pos.x + value.galley.size().x - value_right()).abs() < 0.5,
        "the value keeps the right edge of its row: {} against {}",
        value.pos.x + value.galley.size().x,
        value_right()
    );
}

#[test]
fn a_value_of_several_lines_starts_at_one_left_edge() {
    let first = "The temporary directory of the machine is not writable by the program that the step started.";
    let second = "The provider program answered nothing at all, so the machine was never created for you.";
    let output = kv_card(&[("What failed", first), ("Undone", second)]);

    let one = text_of(&output, first);
    let two = text_of(&output, second);
    assert!(one.galley.rows.len() >= 2 && two.galley.rows.len() >= 2);
    assert!(
        (one.pos.x - two.pos.x).abs() < 0.5,
        "two long values start at the same left edge: {} against {}",
        one.pos.x,
        two.pos.x
    );
}

#[test]
fn a_long_label_wraps_in_its_column_and_leaves_the_value_its_room() {
    let label = "The machine that this browser panel runs on";
    let output = kv_card(&[(label, "helsinki-1")]);

    let drawn = text_of(&output, label);
    assert!(!drawn.galley.elided, "the label is not cut: {drawn:?}");
    let value = text_of(&output, "helsinki-1");
    assert!(
        (value.pos.x + value.galley.size().x - value_right()).abs() < 0.5,
        "the value keeps the right edge of its row beside a long label"
    );
    assert!(
        value.pos.x > drawn.pos.x,
        "the label keeps its own column, and the value keeps the rest of the row"
    );
}

#[test]
fn the_height_estimate_of_a_new_card_counts_the_lines_of_a_value() {
    let long = default_card_size(
        &kv_document(&[(
            "What failed",
            "The temporary directory of the machine is not writable by the program that the step started.",
        )]),
        0.0,
    )[1];
    let short = default_card_size(&kv_document(&[("Cost", "0.012 EUR / hour")]), 0.0)[1];

    assert!(
        long > short,
        "a value of three lines opens a taller card than a value of one line: {long} against {short}"
    );
}

/// The pass of a card 420 points wide that holds one list of `rows`.
fn list_card(rows: &[(&str, &str)]) -> Vec<egui::epaint::ClippedShape> {
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![Component::List {
            selection: Selection::None,
            rows: rows
                .iter()
                .map(|(title, subtitle)| horizon_browser_control::block::ListRow {
                    id: title.to_lowercase().replace(' ', "-"),
                    title: (*title).to_string(),
                    subtitle: Some((*subtitle).to_string()),
                    tone: None,
                    selected: false,
                    value: None,
                    badge: None,
                    disabled: false,
                })
                .collect(),
            layout: ListLayout::Rows,
        }],
    };
    let (output, _) = CardFrames::new().pass_in(420.0, &document, &mut CardRuntime::default(), Owner::Listening);
    output
}

#[test]
fn a_long_second_line_of_a_row_wraps_instead_of_being_cut() {
    let reason = "The machine must stay on while another computer of yours needs the server that it keeps.";
    let output = list_card(&[("This computer", reason), ("helsinki-1", "A machine of your list.")]);

    let subtitle = text_of(&output, reason);
    assert!(!subtitle.galley.elided, "the second line is not cut: {subtitle:?}");
    assert!(
        subtitle.galley.rows.len() >= 2,
        "the second line wraps to the lines it needs: {:?}",
        subtitle.galley.rows.len()
    );
    let next = text_of(&output, "helsinki-1");
    assert!(
        next.pos.y > subtitle.pos.y + subtitle.galley.size().y,
        "the row grew with its second line, so the next row starts below it"
    );
}
