use serde_json::json;

use super::{
    Badge, BlockDocument, BlockDocumentError, Button, Component, KvRow, ListRow, Selection, Tone, parse_document,
    validate_document,
};

/// The example an agent sends for a machine it holds.
const EXAMPLE: &str = r#"{
  "components": [
    {"type": "heading", "text": "cloud-1", "subtitle": "Hetzner · Helsinki",
     "badge": {"text": "Running", "tone": "ok"}},
    {"type": "badge", "text": "running", "tone": "ok"},
    {"type": "kv", "rows": [
      {"label": "State", "value": "running"},
      {"label": "Address", "value": "203.0.113.10", "mono": true},
      {"label": "Region", "value": "eu-west"}]},
    {"type": "text", "text": "Started from the agent's workspace.\nThe build has 4 steps."},
    {"type": "progress", "label": "Hour 3 of 24", "percent": 12, "detail": "Saving"},
    {"type": "divider"},
    {"type": "list", "selection": "single", "rows": [
      {"id": "size.cx22", "title": "CX22 - 2 vCPU", "subtitle": "4 GB memory"},
      {"id": "size.cx32", "title": "CX32 - 4 vCPU",
       "subtitle": "gcloud is not installed", "tone": "warn"}]},
    {"type": "buttons", "items": [
      {"id": "connect", "label": "Connect", "tone": "primary"},
      {"id": "stop", "label": "Stop", "tone": "warn"},
      {"id": "delete", "label": "Delete", "tone": "danger"}]}
  ]
}"#;

fn document_of(components: &[String]) -> String {
    format!("{{\"components\":[{}]}}", components.join(","))
}

fn divider() -> String {
    json!({"type": "divider"}).to_string()
}

fn text_component(length: usize) -> String {
    json!({"type": "text", "text": "a".repeat(length)}).to_string()
}

fn kv_component(rows: usize) -> String {
    let rows: Vec<serde_json::Value> = (0..rows)
        .map(|_| json!({"label": "State", "value": "running"}))
        .collect();
    json!({"type": "kv", "rows": rows}).to_string()
}

fn list_component(rows: usize) -> String {
    let rows: Vec<serde_json::Value> = (0..rows)
        .map(|index| json!({"id": format!("row.{index}"), "title": "Row"}))
        .collect();
    json!({"type": "list", "selection": "single", "rows": rows}).to_string()
}

fn buttons_component(items: usize) -> String {
    let items: Vec<serde_json::Value> = (0..items)
        .map(|index| json!({"id": format!("button{index}"), "label": "Press"}))
        .collect();
    json!({"type": "buttons", "items": items}).to_string()
}

/// A valid document whose serialized form is exactly `target_len` bytes: full
/// text components, then one text component padded to the remaining bytes.
fn padded_document(target_len: usize) -> String {
    let mut components: Vec<String> = Vec::new();
    let mut length = document_of(&[]).len();
    loop {
        let separator = usize::from(!components.is_empty());
        let remaining = target_len
            .checked_sub(length + separator)
            .expect("the padding never outgrows the target");
        let overhead = text_component(0).len();
        if remaining > overhead {
            let padding = remaining - overhead;
            if padding <= BlockDocument::MAX_TEXT_CHARS {
                components.push(text_component(padding));
                break;
            }
        }
        let full = text_component(BlockDocument::MAX_TEXT_CHARS);
        length += separator + full.len();
        components.push(full);
    }
    document_of(&components)
}

#[test]
fn full_example_round_trips() {
    let document = parse_document(EXAMPLE).expect("the example is valid");
    assert_eq!(document.components.len(), 8);
    assert_eq!(
        document.components[0],
        Component::Heading {
            text: "cloud-1".to_owned(),
            subtitle: Some("Hetzner · Helsinki".to_owned()),
            badge: Some(Badge {
                text: "Running".to_owned(),
                tone: Tone::Ok
            })
        }
    );
    assert_eq!(
        document.components[1],
        Component::Badge {
            text: "running".to_owned(),
            tone: Tone::Ok
        }
    );
    let serialized = serde_json::to_string(&document).expect("the document serializes");
    let reparsed = parse_document(&serialized).expect("the serialized document is valid");
    assert_eq!(reparsed, document);
}

#[test]
fn a_hand_built_document_with_every_component_validates() {
    let document = BlockDocument {
        components: vec![
            Component::Heading {
                text: "cloud-1".to_owned(),
                subtitle: Some("Hetzner · Helsinki".to_owned()),
                badge: None,
            },
            Component::Text {
                text: "one\ntwo".to_owned(),
            },
            Component::Kv {
                rows: vec![KvRow {
                    label: "State".to_owned(),
                    value: "running".to_owned(),
                    mono: true,
                }],
            },
            Component::Badge {
                text: "running".to_owned(),
                tone: Tone::Ok,
            },
            Component::Progress {
                label: "Hour 3 of 24".to_owned(),
                percent: 12,
                detail: Some("Saving".to_owned()),
            },
            Component::Divider,
            Component::List {
                selection: Selection::Single,
                rows: vec![ListRow {
                    id: "size.cx22".to_owned(),
                    title: "CX22 - 2 vCPU".to_owned(),
                    subtitle: None,
                    tone: Some(Tone::Warn),
                }],
            },
            Component::Buttons {
                items: vec![Button {
                    id: "stop".to_owned(),
                    label: "Stop".to_owned(),
                    tone: Tone::Warn,
                }],
            },
        ],
    };
    assert_eq!(validate_document(&document), Ok(()));
}

#[test]
fn a_document_at_the_size_limit_parses_and_one_byte_over_is_rejected() {
    let at_limit = padded_document(BlockDocument::MAX_SERIALIZED_BYTES);
    assert_eq!(at_limit.len(), BlockDocument::MAX_SERIALIZED_BYTES);
    let document = parse_document(&at_limit).expect("a document at the limit parses");
    assert!(document.components.len() <= BlockDocument::MAX_COMPONENTS);

    let over = padded_document(BlockDocument::MAX_SERIALIZED_BYTES + 1);
    let error = parse_document(&over).expect_err("a document above the limit is rejected");
    assert_eq!(error.code(), "document_too_large");
    assert_eq!(
        error,
        BlockDocumentError::DocumentTooLarge {
            actual: BlockDocument::MAX_SERIALIZED_BYTES + 1,
            limit: BlockDocument::MAX_SERIALIZED_BYTES
        }
    );
}

#[test]
fn a_document_at_the_component_limit_parses_and_one_over_is_rejected() {
    let at_limit: Vec<String> = (0..BlockDocument::MAX_COMPONENTS).map(|_| divider()).collect();
    let document = parse_document(&document_of(&at_limit)).expect("64 components parse");
    assert_eq!(document.components.len(), BlockDocument::MAX_COMPONENTS);

    let over: Vec<String> = (0..=BlockDocument::MAX_COMPONENTS).map(|_| divider()).collect();
    let error = parse_document(&document_of(&over)).expect_err("65 components are rejected");
    assert_eq!(error.code(), "invalid_document");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components".to_owned(),
            actual: 65,
            limit: 64
        }
    );
}

#[test]
fn kv_rows_at_the_limit_parse_and_one_over_is_rejected() {
    let at_limit = document_of(&[kv_component(BlockDocument::MAX_KV_ROWS)]);
    assert!(parse_document(&at_limit).is_ok());

    let over = document_of(&[kv_component(BlockDocument::MAX_KV_ROWS + 1)]);
    let error = parse_document(&over).expect_err("25 rows are rejected");
    assert_eq!(error.code(), "invalid_document");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].rows".to_owned(),
            actual: 25,
            limit: 24
        }
    );
}

#[test]
fn list_rows_at_the_limit_parse_and_one_over_is_rejected() {
    let at_limit = document_of(&[list_component(BlockDocument::MAX_LIST_ROWS)]);
    assert!(parse_document(&at_limit).is_ok());

    let over = document_of(&[list_component(BlockDocument::MAX_LIST_ROWS + 1)]);
    let error = parse_document(&over).expect_err("33 rows are rejected");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].rows".to_owned(),
            actual: 33,
            limit: 32
        }
    );
}

#[test]
fn buttons_at_the_limit_parse_and_one_over_is_rejected() {
    let at_limit = document_of(&[buttons_component(BlockDocument::MAX_BUTTONS)]);
    assert!(parse_document(&at_limit).is_ok());

    let over = document_of(&[buttons_component(BlockDocument::MAX_BUTTONS + 1)]);
    let error = parse_document(&over).expect_err("9 buttons are rejected");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].items".to_owned(),
            actual: 9,
            limit: 8
        }
    );
}

#[test]
fn text_length_counts_characters_up_to_512() {
    let at_limit = document_of(&[text_component(BlockDocument::MAX_TEXT_CHARS)]);
    assert!(parse_document(&at_limit).is_ok());

    let multibyte = json!({"type": "text", "text": "é".repeat(BlockDocument::MAX_TEXT_CHARS)}).to_string();
    assert!(parse_document(&document_of(&[multibyte])).is_ok());

    let over = document_of(&[text_component(BlockDocument::MAX_TEXT_CHARS + 1)]);
    let error = parse_document(&over).expect_err("513 characters are rejected");
    assert_eq!(
        error,
        BlockDocumentError::TextTooLong {
            path: "components[0].text".to_owned(),
            actual: 513,
            limit: 512
        }
    );
}

#[test]
fn a_single_line_text_above_the_limit_names_its_field() {
    let over = json!({"type": "heading", "text": "a".repeat(BlockDocument::MAX_TEXT_CHARS + 1)}).to_string();
    let error = parse_document(&document_of(&[over])).expect_err("513 characters are rejected");
    assert_eq!(
        error,
        BlockDocumentError::TextTooLong {
            path: "components[0].text".to_owned(),
            actual: 513,
            limit: 512
        }
    );
}

#[test]
fn a_document_without_the_optional_fields_keeps_its_defaults() {
    let plain = json!({
        "components": [
            {"type": "heading", "text": "cloud-1"},
            {"type": "kv", "rows": [{"label": "State", "value": "running"}]},
            {"type": "progress", "label": "Hour 3 of 24", "percent": 12},
            {"type": "list", "selection": "none", "rows": [{"id": "row.1", "title": "Row"}]}
        ]
    })
    .to_string();
    let document = parse_document(&plain).expect("the optional fields may be absent");

    let Component::Heading { subtitle, badge, .. } = &document.components[0] else {
        panic!("a heading");
    };
    assert_eq!((subtitle, badge), (&None, &None));
    let Component::Kv { rows } = &document.components[1] else {
        panic!("a kv");
    };
    assert!(!rows[0].mono, "a value is proportional unless it says otherwise");
    let Component::Progress { detail, .. } = &document.components[2] else {
        panic!("a progress");
    };
    assert_eq!(detail, &None);
    let Component::List { rows, .. } = &document.components[3] else {
        panic!("a list");
    };
    assert_eq!(rows[0].tone, None, "a subtitle is neutral unless it says otherwise");
}

#[test]
fn the_optional_texts_hold_512_characters() {
    let longest = "b".repeat(BlockDocument::MAX_TEXT_CHARS);
    for at_limit in [
        json!({"type": "heading", "text": "a", "subtitle": longest}),
        json!({"type": "heading", "text": "a", "badge": {"text": longest}}),
        json!({"type": "progress", "label": "a", "percent": 1, "detail": longest}),
    ] {
        assert!(
            parse_document(&document_of(&[at_limit.to_string()])).is_ok(),
            "512 characters are accepted: {at_limit}"
        );
    }
}

#[test]
fn an_optional_text_above_the_limit_names_its_field() {
    let long = "b".repeat(BlockDocument::MAX_TEXT_CHARS + 1);
    for (component, path) in [
        (
            json!({"type": "heading", "text": "a", "subtitle": long}),
            "components[0].subtitle",
        ),
        (
            json!({"type": "heading", "text": "a", "badge": {"text": long}}),
            "components[0].badge.text",
        ),
        (
            json!({"type": "progress", "label": "a", "percent": 1, "detail": long}),
            "components[0].detail",
        ),
    ] {
        let error = parse_document(&document_of(&[component.to_string()])).expect_err("513 characters are rejected");
        assert_eq!(
            error,
            BlockDocumentError::TextTooLong {
                path: path.to_owned(),
                actual: BlockDocument::MAX_TEXT_CHARS + 1,
                limit: BlockDocument::MAX_TEXT_CHARS
            }
        );
    }
}

#[test]
fn an_optional_text_rejects_a_control_character_with_its_place() {
    for (component, path, code_point) in [
        (
            json!({"type": "heading", "text": "a", "subtitle": "b\tc"}),
            "components[0].subtitle",
            9,
        ),
        (
            json!({"type": "heading", "text": "a", "badge": {"text": "b\nc"}}),
            "components[0].badge.text",
            10,
        ),
        (
            json!({"type": "progress", "label": "a", "percent": 1, "detail": "b\u{7}c"}),
            "components[0].detail",
            7,
        ),
    ] {
        let error =
            parse_document(&document_of(&[component.to_string()])).expect_err("a control character is rejected");
        assert_eq!(
            error,
            BlockDocumentError::ControlCharacter {
                path: path.to_owned(),
                code_point
            },
            "{path} names the place that broke the rule"
        );
    }
}

#[test]
fn an_unknown_tone_or_a_non_boolean_mono_is_rejected() {
    let tone = json!({"type": "list", "selection": "none", "rows": [
        {"id": "row.1", "title": "Row", "tone": "loud"}
    ]})
    .to_string();
    assert_eq!(
        parse_document(&document_of(&[tone]))
            .expect_err("loud is not a tone")
            .code(),
        "invalid_document"
    );

    let mono = json!({"type": "kv", "rows": [{"label": "State", "value": "running", "mono": "yes"}]}).to_string();
    assert_eq!(
        parse_document(&document_of(&[mono]))
            .expect_err("mono is a boolean")
            .code(),
        "invalid_document"
    );
}

#[test]
fn control_characters_are_rejected_with_the_place() {
    let heading = json!({"type": "heading", "text": "a\u{7}b"}).to_string();
    assert_eq!(
        parse_document(&document_of(&[heading])).expect_err("a bell is rejected"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].text".to_owned(),
            code_point: 7
        }
    );

    let value = json!({"type": "kv", "rows": [{"label": "State", "value": "a\tb"}]}).to_string();
    assert_eq!(
        parse_document(&document_of(&[value])).expect_err("a tab is rejected"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].rows[0].value".to_owned(),
            code_point: 9
        }
    );
}

#[test]
fn a_line_break_is_allowed_only_in_body_text() {
    let body = json!({"type": "text", "text": "one\ntwo"}).to_string();
    assert!(parse_document(&document_of(&[body])).is_ok());

    let heading = json!({"type": "heading", "text": "one\ntwo"}).to_string();
    assert_eq!(
        parse_document(&document_of(&[heading])).expect_err("a heading is one line"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].text".to_owned(),
            code_point: 10
        }
    );

    let carriage_return = json!({"type": "text", "text": "one\rtwo"}).to_string();
    assert_eq!(
        parse_document(&document_of(&[carriage_return])).expect_err("body text holds no carriage return"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].text".to_owned(),
            code_point: 13
        }
    );

    let subtitle = json!({"type": "list", "selection": "none", "rows": [
        {"id": "row.1", "title": "Row", "subtitle": "one\ntwo"}
    ]})
    .to_string();
    assert_eq!(
        parse_document(&document_of(&[subtitle])).expect_err("a subtitle is one line"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].rows[0].subtitle".to_owned(),
            code_point: 10
        }
    );
}

#[test]
fn an_id_at_the_limit_parses_and_one_over_is_rejected() {
    let at_limit =
        json!({"type": "buttons", "items": [{"id": "a".repeat(BlockDocument::MAX_ID_CHARS), "label": "Press"}]})
            .to_string();
    assert!(parse_document(&document_of(&[at_limit])).is_ok());

    let over =
        json!({"type": "buttons", "items": [{"id": "a".repeat(BlockDocument::MAX_ID_CHARS + 1), "label": "Press"}]})
            .to_string();
    let error = parse_document(&document_of(&[over])).expect_err("65 characters are rejected");
    assert_eq!(
        error,
        BlockDocumentError::InvalidId {
            path: "components[0].items[0].id".to_owned(),
            id: "a".repeat(BlockDocument::MAX_ID_CHARS + 1)
        }
    );
}

#[test]
fn an_id_with_a_space_a_capital_or_a_slash_is_rejected() {
    for id in ["Stop", "stop now", "", "stop/now", "é"] {
        let component = json!({"type": "buttons", "items": [{"id": id, "label": "Press"}]}).to_string();
        let error = parse_document(&document_of(&[component])).expect_err("the id is not allowed");
        assert_eq!(
            error,
            BlockDocumentError::InvalidId {
                path: "components[0].items[0].id".to_owned(),
                id: id.to_owned()
            }
        );
    }
}

#[test]
fn a_duplicate_id_across_a_button_and_a_list_row_is_rejected() {
    let list = json!({"type": "list", "selection": "single", "rows": [{"id": "row.1", "title": "Row"}]}).to_string();
    let buttons = json!({"type": "buttons", "items": [{"id": "row.1", "label": "Press"}]}).to_string();
    let error = parse_document(&document_of(&[list, buttons])).expect_err("both use row.1");
    assert_eq!(
        error,
        BlockDocumentError::DuplicateId {
            path: "components[1].items[0].id".to_owned(),
            id: "row.1".to_owned()
        }
    );
}

#[test]
fn validate_document_rejects_a_repeated_button_id() {
    let document = BlockDocument {
        components: vec![Component::Buttons {
            items: vec![
                Button {
                    id: "stop".to_owned(),
                    label: "Stop".to_owned(),
                    tone: Tone::Warn,
                },
                Button {
                    id: "stop".to_owned(),
                    label: "Stop again".to_owned(),
                    tone: Tone::Danger,
                },
            ],
        }],
    };
    assert_eq!(
        validate_document(&document),
        Err(BlockDocumentError::DuplicateId {
            path: "components[0].items[1].id".to_owned(),
            id: "stop".to_owned()
        })
    );
}

#[test]
fn percent_at_the_limit_parses_and_one_over_is_rejected() {
    let at_limit =
        json!({"type": "progress", "label": "Hour 3 of 24", "percent": BlockDocument::MAX_PERCENT}).to_string();
    assert!(parse_document(&document_of(&[at_limit])).is_ok());

    let over = json!({"type": "progress", "label": "Hour 3 of 24", "percent": 101}).to_string();
    let error = parse_document(&document_of(&[over])).expect_err("101 percent is rejected");
    assert_eq!(
        error,
        BlockDocumentError::PercentOutOfRange {
            path: "components[0].percent".to_owned(),
            percent: 101,
            limit: 100
        }
    );
}

#[test]
fn tone_defaults_to_neutral_for_a_badge_and_a_button() {
    let document = parse_document(&document_of(&[
        json!({"type": "badge", "text": "running"}).to_string(),
        json!({"type": "buttons", "items": [{"id": "stop", "label": "Stop"}]}).to_string(),
    ]))
    .expect("tone is optional");
    assert_eq!(
        document.components[0],
        Component::Badge {
            text: "running".to_owned(),
            tone: Tone::Neutral
        }
    );
    assert_eq!(
        document.components[1],
        Component::Buttons {
            items: vec![Button {
                id: "stop".to_owned(),
                label: "Stop".to_owned(),
                tone: Tone::Neutral
            }]
        }
    );
}

#[test]
fn an_unknown_tone_is_rejected() {
    let component = json!({"type": "badge", "text": "running", "tone": "loud"}).to_string();
    let error = parse_document(&document_of(&[component])).expect_err("loud is not a tone");
    assert_eq!(error.code(), "invalid_document");
    assert!(matches!(error, BlockDocumentError::InvalidJson { .. }));
    assert!(error.to_string().contains("unknown variant `loud`"), "{error}");
}

#[test]
fn list_selection_is_none_or_single_and_is_required() {
    for selection in ["none", "single"] {
        let component = json!({"type": "list", "selection": selection, "rows": []}).to_string();
        assert!(
            parse_document(&document_of(&[component])).is_ok(),
            "{selection} is a selection"
        );
    }

    let component = json!({"type": "list", "rows": []}).to_string();
    let error = parse_document(&document_of(&[component])).expect_err("selection is required");
    assert!(error.to_string().contains("missing field `selection`"), "{error}");
}

#[test]
fn an_empty_heading_label_title_or_button_label_is_rejected() {
    let cases = [
        (json!({"type": "heading", "text": ""}).to_string(), "components[0].text"),
        (
            json!({"type": "kv", "rows": [{"label": " ", "value": "running"}]}).to_string(),
            "components[0].rows[0].label",
        ),
        (
            json!({"type": "progress", "label": "", "percent": 12}).to_string(),
            "components[0].label",
        ),
        (
            json!({"type": "list", "selection": "single", "rows": [{"id": "row.1", "title": ""}]}).to_string(),
            "components[0].rows[0].title",
        ),
        (
            json!({"type": "buttons", "items": [{"id": "stop", "label": ""}]}).to_string(),
            "components[0].items[0].label",
        ),
    ];
    for (component, path) in cases {
        let error = parse_document(&document_of(&[component])).expect_err("the text is empty");
        assert_eq!(error.code(), "invalid_document");
        assert_eq!(error, BlockDocumentError::EmptyText { path: path.to_owned() });
    }
}

#[test]
fn an_unknown_field_is_rejected() {
    let error = parse_document(r#"{"components":[{"type":"heading","text":"cloud-1","extra":1}]}"#)
        .expect_err("extra is not a field");
    assert_eq!(error.code(), "invalid_document");
    assert!(matches!(error, BlockDocumentError::InvalidJson { .. }));
    assert!(error.to_string().contains("unknown field `extra`"), "{error}");
}

#[test]
fn an_unknown_component_type_is_rejected() {
    let error = parse_document(r#"{"components":[{"type":"chart"}]}"#).expect_err("chart is not a component");
    assert!(matches!(error, BlockDocumentError::InvalidJson { .. }));
    assert!(error.to_string().contains("unknown variant `chart`"), "{error}");
}

#[test]
fn a_revision_is_not_part_of_the_document() {
    let error = parse_document(r#"{"revision":7,"components":[]}"#).expect_err("the host adds the revision");
    assert_eq!(error.code(), "invalid_document");
    assert!(error.to_string().contains("unknown field `revision`"), "{error}");
}
