//! The lines of a card do not move with the canvas zoom.
//!
//! A job of this toolkit breaks its lines on the pixel-rounded position of a
//! glyph, so the same text can break at one zoom and not at another. Every
//! component of a card therefore keeps the lines of zoom 1: this file draws the
//! card documents that Horizon ships and the notice card at forty zooms, and
//! compares every painted line with the line that zoom 1 drew.

use egui::epaint::{ClippedShape, Shape, TextShape};
use horizon_browser_control::block::{BlockDocument, parse_document};
use horizon_core::CardRuntime;

use super::{CardFrames, Owner};
use crate::app::setup::notice::{self, ProgramRow};

/// The card documents of the bundled skills, from the copy that the build
/// embeds. The two bundled copies of each skill hold the same bytes, and the
/// coverage check keeps them in step.
const SKILLS: [(&str, &str); 5] = [
    (
        "horizon-setup",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-setup/SKILL.md"
        )),
    ),
    (
        "horizon-server",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-server/SKILL.md"
        )),
    ),
    (
        "horizon-machines",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/SKILL.md"
        )),
    ),
    (
        "horizon-plan",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-plan/SKILL.md"
        )),
    ),
    (
        "horizon-server-move",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-server-move/SKILL.md"
        )),
    ),
];

/// Whether one fenced block of a skill is a card document: a document of
/// components, with or without the width that it asks for.
fn is_card_document(block: &str) -> bool {
    block.contains("\"components\"") && (block.starts_with(r#"{"components""#) || block.starts_with(r#"{"width""#))
}

/// Every fenced JSON block of one skill that is a card document.
fn skill_documents(skill: &str, body: &str) -> Vec<BlockDocument> {
    let mut documents = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("```json\n") {
        let after = &rest[start + "```json\n".len()..];
        let Some(end) = after.find("```") else {
            panic!("{skill}: a fenced JSON block does not close");
        };
        let block = after[..end].trim();
        rest = &after[end + 3..];
        if !is_card_document(block) {
            continue;
        }
        documents.push(parse_document(block).unwrap_or_else(|error| panic!("{skill}: {error}\n{block}")));
    }
    documents
}

/// The card of the notice that Set up draws when it finds no agent, with the
/// programs that it looked for.
fn notice_document() -> BlockDocument {
    let rows = [("Claude", "claude"), ("Codex", "codex")]
        .into_iter()
        .map(|(name, program)| ProgramRow {
            name: name.to_owned(),
            program: program.to_owned(),
            found: false,
        })
        .collect::<Vec<_>>();
    notice::document(&rows, true)
}

/// Every document under test, named for a message.
fn documents() -> Vec<(String, BlockDocument)> {
    let mut documents = Vec::new();
    for (skill, body) in SKILLS {
        for (index, document) in skill_documents(skill, body).into_iter().enumerate() {
            documents.push((format!("{skill} card {index}"), document));
        }
    }
    documents.push(("the notice".to_owned(), notice_document()));
    documents
}

/// The words of every line that `shapes` painted, in the order they were
/// painted, with the lines of each text in order.
fn painted_lines(shapes: &[ClippedShape]) -> Vec<(String, Vec<String>)> {
    fn walk(shape: &Shape, lines: &mut Vec<(String, Vec<String>)>) {
        match shape {
            Shape::Text(text) => lines.push(painted_text(text)),
            Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, lines);
                }
            }
            _ => {}
        }
    }

    let mut lines = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut lines);
    }
    lines
}

/// The lines of one painted text, as the words of each row.
fn painted_text(text: &TextShape) -> (String, Vec<String>) {
    let rows = text
        .galley
        .rows
        .iter()
        .map(|placed| {
            placed
                .row
                .glyphs
                .iter()
                .map(|glyph| glyph.chr)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    // The label of the text, with the whitespace of a break made one space, so
    // that a break does not show up as a difference of the label.
    let label = text.galley.text().split_whitespace().collect::<Vec<_>>().join(" ");
    (label, rows)
}

/// The lines of the documents at `zoom`, each one drawn by the card frames of
/// that zoom. The first pass settles the glyph pictures of the frames to the
/// zoom, so the passes under test draw the text of the screen size.
fn lines_at_zoom(zoom: f32, documents: &[(String, BlockDocument)]) -> Vec<Vec<(String, Vec<String>)>> {
    let frames = CardFrames::new();
    let mut warm_up = CardRuntime::default();
    let _ = frames.paint_at(zoom, &documents[0].1, &mut warm_up, Owner::Listening);
    documents
        .iter()
        .map(|(_, document)| {
            let mut card = CardRuntime::default();
            let output = frames.paint_at(zoom, document, &mut card, Owner::Listening);
            painted_lines(&output.shapes)
        })
        .collect()
}

/// Every component of the card documents of the bundled skills and of the notice
/// card keeps the line count and the words of every line of zoom 1, at forty
/// zoom values between 0.5 and 2.0.
#[test]
fn a_card_keeps_the_lines_of_zoom_one_at_forty_zooms() {
    let documents = documents();
    assert!(documents.len() > 20, "the skills ship {} cards", documents.len());
    let expected = lines_at_zoom(1.0, &documents);
    assert!(
        expected.iter().all(|lines| !lines.is_empty()),
        "every card paints a text"
    );
    assert!(
        expected
            .iter()
            .any(|lines| lines.iter().any(|(_, rows)| rows.len() > 1)),
        "a card of the set breaks a text over more than one line"
    );
    for step in 0..40 {
        #[allow(clippy::cast_precision_loss)]
        let zoom = 0.5 + step as f32 * (1.5 / 39.0);
        let drawn = lines_at_zoom(zoom, &documents);
        for ((name, _), (drawn, expected)) in documents.iter().zip(drawn.iter().zip(&expected)) {
            assert_eq!(drawn, expected, "{name} keeps the lines of zoom 1 at zoom {zoom}");
        }
    }
}
