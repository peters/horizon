//! The card documents of the bundled skills, and the rules that they keep.
//!
//! A skill draws its cards as fenced JSON documents, so the documents are the
//! examples that an agent copies. This module parses them for the tests of the
//! card body: the lines of every document keep the lines of zoom 1, and every
//! board of a document keeps the tag and the value of each of its columns.

use horizon_browser_control::block::{BlockDocument, Component, parse_document};

/// The card documents of the bundled skills, from the copy that the build
/// embeds. The two bundled copies of each skill hold the same bytes, and the
/// coverage check keeps them in step.
const SKILLS: [(&str, &str); 7] = [
    (
        "horizon-setup",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-setup/SKILL.md"
        )),
    ),
    (
        "horizon-setup/workshop",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-setup/references/workshop.md"
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
        "horizon-plan/from-an-issue",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-plan/references/from-an-issue.md"
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

/// Every card document of the bundled skills, named for a message.
pub(super) fn skill_cards() -> Vec<(String, BlockDocument)> {
    let mut documents = Vec::new();
    for (skill, body) in SKILLS {
        for (index, document) in skill_documents(skill, body).into_iter().enumerate() {
            documents.push((format!("{skill} card {index}"), document));
        }
    }
    documents
}

/// Every board of the card documents of the skills, named for a message, with the
/// document that holds it.
fn boards() -> Vec<(String, BlockDocument, usize)> {
    let mut boards = Vec::new();
    for (name, document) in skill_cards() {
        for (index, component) in document.components.iter().enumerate() {
            if matches!(component, Component::Board { .. }) {
                boards.push((name.clone(), document.clone(), index));
            }
        }
    }
    boards
}

/// A plan card is the account of a wave and of the work of one todo in it, so
/// every column of every board of a skill names its wave in `tag` and its time in
/// `value`, and every item of the board names its time in `value`. A column that
/// loses its tag loses the number of the wave; a column or an item that loses its
/// value loses the estimate or the real time of the work.
#[test]
fn every_column_of_a_board_of_the_skills_holds_a_tag_and_a_value() {
    let boards = boards();
    assert!(boards.len() > 5, "the skills ship {} boards", boards.len());
    for (name, document, index) in boards {
        let Component::Board { columns, items, .. } = &document.components[index] else {
            continue;
        };
        for (column, entry) in columns.iter().enumerate() {
            assert!(
                entry.tag.as_deref().is_some_and(|tag| !tag.trim().is_empty()),
                "{name}: column {column} of the board names no tag"
            );
            assert!(
                entry.value.as_deref().is_some_and(|value| !value.trim().is_empty()),
                "{name}: column {column} of the board names no value"
            );
        }
        for (item, entry) in items.iter().enumerate() {
            assert!(
                entry.value.as_deref().is_some_and(|value| !value.trim().is_empty()),
                "{name}: item {item} of the board names no value"
            );
        }
    }
}

/// The first card of Set up is the entry of the whole workshop: this version
/// adds no tile to the first start page, so the row `choose.workshop` of that
/// card is the way to the goal `workshop`.
#[test]
fn the_first_card_of_set_up_offers_the_whole_workshop_as_a_row() {
    let offers = skill_cards().iter().any(|(name, document)| {
        name.starts_with("horizon-setup")
            && document.components.iter().any(|component| match component {
                Component::List { rows, .. } => rows.iter().any(|row| row.id == "choose.workshop"),
                _ => false,
            })
    });
    assert!(
        offers,
        "the first card of Set up lists the goal workshop as a row, because the start page has no tile for it"
    );
}
