//! The card documents of the bundled skills, and the rules that they keep.
//!
//! A skill draws its cards as fenced JSON documents, so the documents are the
//! examples that an agent copies. This module parses them for the tests of the
//! card body: the lines of every document keep the lines of zoom 1, and every
//! board of a document keeps the tag and the value of each of its columns.

use horizon_browser_control::block::{BlockDocument, Component, Selection, Tone, parse_document};

/// The card documents of the bundled skills, from the copy that the build
/// embeds. The two bundled copies of each skill hold the same bytes, and the
/// coverage check keeps them in step.
const SKILLS: [(&str, &str); 10] = [
    (
        "horizon-block",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-block/SKILL.md"
        )),
    ),
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
        "horizon-machines/repository-machine",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/repository-machine.md"
        )),
    ),
    (
        "horizon-machines/tailscale",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md"
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

/// The heading of one card, when it has one.
fn heading_of(document: &BlockDocument) -> Option<&str> {
    document.components.iter().find_map(|component| match component {
        Component::Heading { text, .. } => Some(text.as_str()),
        _ => None,
    })
}

/// The ids of the rows of every list of one card, with the selected row first.
fn rows_of(document: &BlockDocument) -> Vec<(String, bool)> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::List { rows, .. } => Some(rows),
            _ => None,
        })
        .flatten()
        .map(|row| (row.id.clone(), row.selected))
        .collect()
}

/// The label and the value of each key and value row of one card.
fn kv_of(document: &BlockDocument) -> Vec<(&str, &str)> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Kv { rows } => Some(rows),
            _ => None,
        })
        .flatten()
        .map(|row| (row.label.as_str(), row.value.as_str()))
        .collect()
}

/// The labels of the steps of one card.
fn steps_of(document: &BlockDocument) -> Vec<&str> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Steps { steps } => Some(steps),
            _ => None,
        })
        .flatten()
        .map(|step| step.label.as_str())
        .collect()
}

/// The body text of every `text` component of one card.
fn texts_of(document: &BlockDocument) -> Vec<&str> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The id of every button of one card.
fn buttons_of(document: &BlockDocument) -> Vec<&str> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Buttons { items } => Some(items),
            _ => None,
        })
        .flatten()
        .map(|button| button.id.as_str())
        .collect()
}

/// The card of the skills that holds this text, named for a message.
fn card_with_text<'a>(cards: &'a [(String, BlockDocument)], wanted: &str) -> (&'a str, &'a BlockDocument) {
    let (name, document) = cards
        .iter()
        .find(|(_, document)| texts_of(document).contains(&wanted))
        .unwrap_or_else(|| panic!("no card of the skills says {wanted:?}"));
    (name.as_str(), document)
}

/// A machine joins one stored network, and the person can store more than one.
/// The card of the join offers `None` first and then the names of the panel, the
/// card that the person needs first names the three secrets and asks for no value,
/// the steps of the join name the network, and the card of a machine that joined
/// keeps the rows of its network and the line of a computer that is not in it.
#[test]
fn the_cards_of_a_network_offer_the_stored_networks_and_name_the_network_of_a_machine() {
    let cards = skill_cards();

    // The join card: `None` first and selected, then the stored networks by name.
    let (name, join) = card_with_text(
        &cards,
        "The machine takes the tag of the network and its own name. The node leaves the network by itself when this machine is deleted.",
    );
    assert_eq!(
        heading_of(join),
        Some("Join build-1 to a network"),
        "{name}: the join card"
    );
    assert_eq!(
        rows_of(join),
        vec![
            ("network.none".to_string(), true),
            ("network.home".to_string(), false),
            ("network.factory".to_string(), false),
        ],
        "{name}: the join card offers None first and then the stored networks"
    );
    assert!(
        buttons_of(join).contains(&"join"),
        "{name}: the join card holds the press"
    );

    // The card that the person needs when no network is stored: the three names and
    // the place of the OAuth client, with no field for a value.
    let (name, store) = card_with_text(
        &cards,
        "Make one OAuth client in the Tailscale admin console, at Trust credentials, OAuth. Add the three names in Settings, Secrets, with Add. Then open a new agent panel, or start Set Up again, and press Join a network.",
    );
    let rows = kv_of(store);
    for secret in ["TS_<NET>_CLIENT_ID", "TS_<NET>_CLIENT_SECRET", "TS_<NET>_TAG"] {
        assert!(
            rows.iter().any(|(label, _)| *label == secret),
            "{name}: the store card names {secret}"
        );
    }
    assert!(
        rows.iter().any(|(_, value)| *value == "Settings, Secrets"),
        "{name}: the store card says where the names go"
    );
    assert_eq!(
        buttons_of(store),
        vec!["close"],
        "{name}: the store card asks for no value"
    );

    // The steps of a join name the network, after the key and before the proof.
    let (name, steps) = card_with_text(
        &cards,
        "The key arrives on standard input and leaves no file on the machine. Port 22 stays open until the proof answers.",
    );
    let labels = steps_of(steps);
    assert!(
        labels.contains(&"Join the network home"),
        "{name}: the steps card of a join names the network: {labels:?}"
    );
    assert!(
        labels.contains(&"Install the client on the machine") && labels.contains(&"Prove the login over the network"),
        "{name}: the join comes after the client and before the proof: {labels:?}"
    );

    // The machine of a computer that is not in the network: the rows of the
    // network, and the line that says which route reaches the machine.
    let (name, machine) = card_with_text(
        &cards,
        "This computer is not in the network home; the machine is reached over its public address.",
    );
    let rows = kv_of(machine);
    for row in ["Network", "Network name", "Internet SSH"] {
        assert!(
            rows.iter().any(|(label, _)| *label == row),
            "{name}: the card of a machine of a network holds the row {row}"
        );
    }

    // The ready card of a machine that a recipe prepared names the network too.
    let (name, ready) = card_with_text(&cards, "cargo test --workspace: 412 tests pass, 0 failed");
    assert!(
        kv_of(ready).iter().any(|(label, _)| *label == "Network"),
        "{name}: the ready card holds the row Network"
    );
    assert!(
        steps_of(ready).iter().any(|step| step.starts_with("Join the network")),
        "{name}: the steps card of a recipe holds the join as its own step"
    );
}

/// The card documents of one skill that the skill marks as end cards: a line
/// `<!-- end card -->` of its own, directly before the fence of the document.
fn end_cards(skill: &str, body: &str) -> Vec<BlockDocument> {
    let mut documents = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "<!-- end card -->" {
            continue;
        }
        let fence = lines[index + 1..]
            .iter()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim())
            .unwrap_or_default();
        assert_eq!(
            fence, "```json",
            "{skill}: the line {index} marks an end card and no card document follows it"
        );
        let rest = lines[index + 1..].join("\n");
        let start = rest.find("```json\n").expect("the fence of the card") + "```json\n".len();
        let after = &rest[start..];
        let end = after.find("```").expect("the fence of the card closes");
        documents.push(
            parse_document(after[..end].trim())
                .unwrap_or_else(|error| panic!("{skill}: the end card {index} does not parse: {error}")),
        );
    }
    documents
}

/// A card that ends a flow holds the same last button, so a run never ends on a
/// card that offers the person no press. The ready card of the browser goal held
/// `Close` in one run and no `Close` in another, so the rule is checked on every
/// end card that a skill marks.
#[test]
fn every_end_card_of_the_skills_ends_with_clos() {
    let mut cards = 0;
    for (skill, body) in SKILLS {
        for (index, document) in end_cards(skill, body).into_iter().enumerate() {
            let ids = buttons_of(&document);
            assert_eq!(
                ids.last().copied(),
                Some("close"),
                "{skill} end card {index} {} does not end with the button close: {ids:?}",
                heading_of(&document).unwrap_or("<no heading>")
            );
            cards += 1;
        }
    }
    assert!(cards >= 5, "the skills mark their end cards, and {cards} were found");
}

/// The label of every button of one card, and of every answer of its callouts, in
/// draw order.
fn button_labels(document: &BlockDocument) -> Vec<&str> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Buttons { items } => Some(items),
            Component::Callout { buttons, .. } => Some(buttons),
            _ => None,
        })
        .flatten()
        .map(|button| button.label.as_str())
        .collect()
}

/// The labels of the buttons of one card that are drawn as its primary action.
fn primary_labels(document: &BlockDocument) -> Vec<&str> {
    document
        .components
        .iter()
        .filter_map(|component| match component {
            Component::Buttons { items } => Some(items),
            Component::Callout { buttons, .. } => Some(buttons),
            _ => None,
        })
        .flatten()
        .filter(|button| button.tone == Tone::Primary)
        .map(|button| button.label.as_str())
        .collect()
}

/// Whether one card asks the person something: it holds a callout with an answer,
/// or a list whose rows the person can choose.
fn asks_a_question(document: &BlockDocument) -> bool {
    document.components.iter().any(|component| match component {
        Component::Callout { buttons, .. } => !buttons.is_empty(),
        Component::List {
            selection: Selection::Single,
            rows,
            ..
        } => rows.iter().any(|row| !row.disabled),
        _ => false,
    })
}

/// Every string of one card, in document order.
fn strings_of(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(text) => vec![text.clone()],
        serde_json::Value::Array(items) => items.iter().flat_map(strings_of).collect(),
        serde_json::Value::Object(fields) => fields.values().flat_map(strings_of).collect(),
        _ => Vec::new(),
    }
}

/// The money units that a card can name.
const MONEY_UNITS: [&str; 7] = ["EUR", "USD", "GBP", "CHF", "NOK", "SEK", "DKK"];

/// A card that a person reads keeps four rules, and a skill that draws such a
/// card keeps them too: one label per button, one primary action on a card that
/// asks something, a price that says its unit and its hour, and a text inside the
/// bound of the host.
#[test]
fn the_cards_of_the_skills_keep_the_rules_of_a_card() {
    for (name, document) in skill_cards() {
        let labels = button_labels(&document);
        let mut seen: Vec<&str> = Vec::new();
        for label in &labels {
            assert!(
                !seen.contains(label),
                "{name}: two buttons of the card say {label:?}: {labels:?}"
            );
            seen.push(label);
        }

        if asks_a_question(&document) {
            let primary = primary_labels(&document);
            assert!(
                primary.len() <= 1,
                "{name}: a card that asks one question holds one primary action, and this one holds {primary:?}"
            );
        }

        let strings = strings_of(&serde_json::to_value(&document).expect("a card serializes"));
        let text = strings.join(" ").to_lowercase();
        for unit in MONEY_UNITS {
            if strings.iter().any(|string| string.contains(unit)) {
                assert!(
                    text.contains("hour"),
                    "{name}: the card names a price in {unit} and no hour, so the person cannot tell the period"
                );
                assert!(
                    text.contains(&unit.to_lowercase()),
                    "{name}: a card that names money names the unit of the amount"
                );
            }
        }

        let longest = strings.iter().map(|string| string.chars().count()).max().unwrap_or(0);
        assert!(
            longest <= 512,
            "{name}: a text of the card holds {longest} characters, above the 512 that the host accepts"
        );
    }
}
