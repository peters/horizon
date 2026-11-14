use serde_json::json;

use super::{
    Badge, BlockDocument, BlockDocumentError, Button, Component, KvRow, ListLayout, ListRow, Selection, Step,
    StepState, Tone, parse_document, validate_document,
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
      {"id": "delete", "label": "Delete", "tone": "danger"}]},
    {"type": "link", "label": "Open session", "url": "https://cloud.example.test/sessions/7"}
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

fn link_component(label: &str, url: &str) -> String {
    json!({"type": "link", "label": label, "url": url}).to_string()
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
    assert_eq!(document.components.len(), 9);
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
        width: None,
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
                    selected: false,
                    value: Some("0.42 EUR/h".to_owned()),
                    badge: None,
                    disabled: false,
                }],
                layout: ListLayout::Rows,
            },
            Component::Buttons {
                items: vec![Button {
                    id: "stop".to_owned(),
                    label: "Stop".to_owned(),
                    tone: Tone::Warn,
                }],
            },
            Component::Link {
                label: "Open session".to_owned(),
                url: "https://cloud.example.test/sessions/7".to_owned(),
            },
            Component::Caption {
                text: "Size".to_owned(),
            },
            Component::Steps {
                steps: vec![Step {
                    label: "Create the machine at the provider".to_owned(),
                    state: StepState::Now,
                    at: Some("11 s".to_owned()),
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
fn a_selected_row_is_allowed_once_in_a_selectable_list_only() {
    let selected = json!({"type": "list", "selection": "single", "rows": [
        {"id": "row.1", "title": "One"},
        {"id": "row.2", "title": "Two", "selected": true}]})
    .to_string();
    let document = parse_document(&document_of(&[selected])).expect("a selected row parses");
    let Some(Component::List { rows, .. }) = document.components.first() else {
        panic!("the list parsed");
    };
    assert!(rows[1].selected);
    assert!(!rows[0].selected, "a row without the field is not selected");

    let two_selected = json!({"type": "list", "selection": "single", "rows": [
        {"id": "row.1", "title": "One", "selected": true},
        {"id": "row.2", "title": "Two", "selected": true}]})
    .to_string();
    let error = parse_document(&document_of(&[two_selected])).expect_err("one row per list selects");
    assert_eq!(error.code(), "invalid_document");
    assert_eq!(
        error,
        BlockDocumentError::DuplicateSelectedRow {
            path: "components[0].rows[1].selected".to_owned(),
            first: "components[0].rows[0].selected".to_owned()
        }
    );

    let unselectable = json!({"type": "list", "selection": "none", "rows": [
        {"id": "row.1", "title": "One", "selected": true}]})
    .to_string();
    let error = parse_document(&document_of(&[unselectable])).expect_err("a list without a selection takes none");
    assert_eq!(error.code(), "invalid_document");
    assert_eq!(
        error,
        BlockDocumentError::RowSelectedInUnselectableList {
            path: "components[0].rows[0].selected".to_owned()
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
fn a_link_parses_into_its_label_and_address() {
    let document = parse_document(&document_of(&[link_component(
        "Open session",
        "https://cloud.example.test/sessions/7",
    )]))
    .expect("a link parses");
    assert_eq!(
        document.components[0],
        Component::Link {
            label: "Open session".to_owned(),
            url: "https://cloud.example.test/sessions/7".to_owned()
        }
    );
}

#[test]
fn links_at_the_limit_parse_and_one_over_is_rejected() {
    let links: Vec<String> = (0..BlockDocument::MAX_LINKS)
        .map(|index| link_component("Open session", &format!("https://example.test/session/{index}")))
        .collect();
    assert!(parse_document(&document_of(&links)).is_ok());

    let mut over = links;
    over.push(link_component("Open session", "https://example.test/session/9"));
    let error = parse_document(&document_of(&over)).expect_err("nine links are rejected");
    assert_eq!(error.code(), "invalid_document");
    assert_eq!(
        error,
        BlockDocumentError::TooManyLinks {
            path: "components[8]".to_owned(),
            actual: 9,
            limit: 8
        }
    );
}

#[test]
fn a_link_label_holds_80_characters_and_an_address_2048() {
    let label = "a".repeat(BlockDocument::MAX_LINK_LABEL_CHARS);
    let base = "https://example.test/";
    let url = format!("{base}{}", "p".repeat(BlockDocument::MAX_LINK_URL_CHARS - base.len()));
    assert_eq!(url.chars().count(), BlockDocument::MAX_LINK_URL_CHARS);
    assert!(parse_document(&document_of(&[link_component(&label, &url)])).is_ok());

    let long_label = "a".repeat(BlockDocument::MAX_LINK_LABEL_CHARS + 1);
    let error = parse_document(&document_of(&[link_component(&long_label, &url)])).expect_err("81 characters");
    assert_eq!(
        error,
        BlockDocumentError::TextTooLong {
            path: "components[0].label".to_owned(),
            actual: BlockDocument::MAX_LINK_LABEL_CHARS + 1,
            limit: BlockDocument::MAX_LINK_LABEL_CHARS
        }
    );

    let long_url = "u".repeat(BlockDocument::MAX_LINK_URL_CHARS + 1);
    let error = parse_document(&document_of(&[link_component("Open session", &long_url)])).expect_err("2049");
    assert_eq!(
        error,
        BlockDocumentError::TextTooLong {
            path: "components[0].url".to_owned(),
            actual: BlockDocument::MAX_LINK_URL_CHARS + 1,
            limit: BlockDocument::MAX_LINK_URL_CHARS
        }
    );
}

#[test]
fn a_link_needs_a_label_and_an_address() {
    for (component, path) in [
        (link_component("", "https://example.test/"), "components[0].label"),
        (link_component("  ", "https://example.test/"), "components[0].label"),
        (link_component("Open session", ""), "components[0].url"),
        (link_component("Open session", " "), "components[0].url"),
    ] {
        let error = parse_document(&document_of(&[component])).expect_err("both fields are filled");
        assert_eq!(error.code(), "invalid_document");
        assert_eq!(error, BlockDocumentError::EmptyText { path: path.to_owned() });
    }
}

#[test]
fn a_link_address_that_horizon_would_not_open_names_its_field() {
    for url in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,<b>x</b>",
        "example.test/session/7",
        "https://",
        "https://user:pass@example.test/session/7",
        "https://user@example.test/session/7",
        "https://example.test/a b",
        " https://example.test/session/7",
    ] {
        let error = parse_document(&document_of(&[link_component("Open session", url)]))
            .expect_err("Horizon opens only a plain page address");
        assert_eq!(error.code(), "invalid_document", "{url:?}");
        assert_eq!(
            error,
            BlockDocumentError::InvalidLinkUrl {
                path: "components[0].url".to_owned()
            },
            "{url:?}"
        );
        assert!(
            !error.to_string().contains("pass"),
            "the answer repeats no part of the address: {error}"
        );
    }
}

#[test]
fn a_link_rejects_a_control_character_with_its_place() {
    let url = "https://example.test/session\u{7}7";
    let error = parse_document(&document_of(&[link_component("Open session", url)])).expect_err("no control");
    assert_eq!(
        error,
        BlockDocumentError::ControlCharacter {
            path: "components[0].url".to_owned(),
            code_point: 7
        }
    );
    let error = parse_document(&document_of(&[link_component(
        "Open\tsession",
        "https://example.test/",
    )]))
    .expect_err("a label is one line");
    assert_eq!(
        error,
        BlockDocumentError::ControlCharacter {
            path: "components[0].label".to_owned(),
            code_point: 9
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
        width: None,
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

#[test]
fn a_row_carries_a_value_a_badge_and_a_state_that_takes_no_choice() {
    let document = parse_document(&document_of(&[
        json!({"type": "list", "selection": "single", "layout": "pills", "rows": [
            {"id": "size.small", "title": "Small", "subtitle": "2 vCPU, 4 GB.",
             "value": "0.006 EUR/h", "badge": {"text": "Ready", "tone": "ok"}, "selected": true},
            {"id": "provider.gcloud", "title": "Google Cloud",
             "subtitle": "Not signed in. Run gcloud auth login", "disabled": true}
        ]})
        .to_string(),
    ]))
    .expect("a row holds a value, a badge and a disabled state");

    let Component::List {
        selection,
        rows,
        layout,
    } = &document.components[0]
    else {
        panic!("a list");
    };
    assert_eq!((*selection, *layout), (Selection::Single, ListLayout::Pills));
    assert_eq!(rows[0].value.as_deref(), Some("0.006 EUR/h"));
    assert_eq!(
        rows[0].badge,
        Some(Badge {
            text: "Ready".to_owned(),
            tone: Tone::Ok
        })
    );
    assert!(rows[0].selected, "the default choice is the row its document selects");
    assert!(!rows[0].disabled);
    assert!(!rows[1].selected, "the row without a selected field is not chosen");
    assert_eq!(rows[1].value, None);
    assert_eq!(rows[1].badge, None);
    assert!(rows[1].disabled, "a row that is not signed in cannot be chosen");
}

#[test]
fn a_list_draws_its_rows_one_under_another_unless_it_says_otherwise() {
    let plain = json!({"type": "list", "selection": "none", "rows": [{"id": "row.1", "title": "Row"}]}).to_string();
    let document = parse_document(&document_of(&[plain])).expect("a list without a layout");
    let Component::List { layout, .. } = &document.components[0] else {
        panic!("a list");
    };
    assert_eq!(*layout, ListLayout::Rows);

    let unknown = json!({"type": "list", "selection": "none", "layout": "grid", "rows": []}).to_string();
    let error = parse_document(&document_of(&[unknown])).expect_err("grid is not a layout");
    assert!(error.to_string().contains("unknown variant `grid`"), "{error}");
}

#[test]
fn a_caption_is_one_filled_line() {
    let caption = json!({"type": "caption", "text": "SIZE"}).to_string();
    let document = parse_document(&document_of(&[caption])).expect("a caption");
    assert_eq!(
        document.components[0],
        Component::Caption {
            text: "SIZE".to_owned()
        }
    );

    let empty = json!({"type": "caption", "text": " "}).to_string();
    assert_eq!(
        parse_document(&document_of(&[empty])).expect_err("a caption is filled"),
        BlockDocumentError::EmptyText {
            path: "components[0].text".to_owned()
        }
    );

    let two_lines = json!({"type": "caption", "text": "SIZE\nPLACE"}).to_string();
    assert_eq!(
        parse_document(&document_of(&[two_lines])).expect_err("a caption is one line"),
        BlockDocumentError::ControlCharacter {
            path: "components[0].text".to_owned(),
            code_point: 10
        }
    );
}

#[test]
fn a_step_holds_a_label_a_state_and_a_short_time() {
    let steps = json!({"type": "steps", "steps": [
        {"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"},
        {"label": "Check tmux on the machine", "state": "now", "at": "now"},
        {"label": "Add the machine to Machines"}
    ]})
    .to_string();
    let document = parse_document(&document_of(&[steps])).expect("a step list");
    let Component::Steps { steps } = &document.components[0] else {
        panic!("steps");
    };
    assert_eq!(steps[0].state, StepState::Done);
    assert_eq!(steps[0].at.as_deref(), Some("2 s"));
    assert_eq!(steps[1].state, StepState::Now);
    assert_eq!(
        steps[2].state,
        StepState::Waiting,
        "a step waits unless it says otherwise"
    );
    assert_eq!(steps[2].at, None);

    for (step, path) in [
        (
            json!({"type": "steps", "steps": [{"label": ""}]}).to_string(),
            "components[0].steps[0].label",
        ),
        (
            json!({"type": "steps", "steps": [{"label": "Wait", "at": "a\tb"}]}).to_string(),
            "components[0].steps[0].at",
        ),
    ] {
        let error = parse_document(&document_of(&[step])).expect_err("the step text is not allowed");
        assert_eq!(error.code(), "invalid_document", "{path}");
    }

    let unknown = json!({"type": "steps", "steps": [{"label": "Wait", "icon": "clock"}]}).to_string();
    let error = parse_document(&document_of(&[unknown])).expect_err("a step holds no icon");
    assert!(error.to_string().contains("unknown field `icon`"), "{error}");
}

#[test]
fn steps_at_the_limit_parse_and_one_over_is_rejected() {
    let steps = |count: usize| {
        let steps: Vec<serde_json::Value> = (0..count).map(|_| json!({"label": "Create the machine"})).collect();
        json!({"type": "steps", "steps": steps}).to_string()
    };
    assert!(parse_document(&document_of(&[steps(BlockDocument::MAX_STEPS)])).is_ok());

    let error = parse_document(&document_of(&[steps(BlockDocument::MAX_STEPS + 1)])).expect_err("one over");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].steps".to_owned(),
            actual: BlockDocument::MAX_STEPS + 1,
            limit: BlockDocument::MAX_STEPS
        }
    );
}

#[test]
fn the_value_and_the_badge_of_a_row_hold_512_characters_and_no_control_character() {
    let longest = "b".repeat(BlockDocument::MAX_TEXT_CHARS);
    for row in [
        json!({"id": "row.1", "title": "Row", "value": longest}),
        json!({"id": "row.1", "title": "Row", "badge": {"text": longest}}),
    ] {
        let document = json!({"type": "list", "selection": "none", "rows": [row]}).to_string();
        assert!(
            parse_document(&document_of(&[document])).is_ok(),
            "512 characters are accepted: {row}"
        );
    }

    let long = "b".repeat(BlockDocument::MAX_TEXT_CHARS + 1);
    for (row, path) in [
        (
            json!({"id": "row.1", "title": "Row", "value": long}),
            "components[0].rows[0].value",
        ),
        (
            json!({"id": "row.1", "title": "Row", "badge": {"text": long}}),
            "components[0].rows[0].badge.text",
        ),
        (
            json!({"id": "row.1", "title": "Row", "value": "a\nb"}),
            "components[0].rows[0].value",
        ),
    ] {
        let error = parse_document(&document_of(&[
            json!({"type": "list", "selection": "none", "rows": [row]}).to_string(),
        ]))
        .expect_err("the value is one line of at most 512 characters");
        assert!(error.to_string().contains(path), "{path}: {error}");
    }
}

/// The four card documents of the Set up test procedure are valid cards, so the
/// person who runs that procedure draws the mock-up as it is written. The
/// procedure is an input of this test: a document that drifts from the rules of
/// a card fails here.
#[test]
fn the_card_documents_of_the_set_up_procedure_are_valid() {
    let procedure = include_str!("../../../../docs/testing/procedures/set-up-wizard.md");
    let mut documents = Vec::new();
    for line in procedure.lines() {
        let request = line.trim();
        if !request.starts_with(r#"{"tool": "block_panel""#) {
            continue;
        }
        let request: serde_json::Value = serde_json::from_str(request).expect("the request of the procedure is JSON");
        let document = request
            .pointer("/arguments/document")
            .expect("the request of the procedure carries a document");
        let encoded = serde_json::to_string(document).expect("the document serializes");
        let parsed = parse_document(&encoded).unwrap_or_else(|error| panic!("{request}: {error}"));
        documents.push(parsed);
    }

    assert_eq!(documents.len(), 4, "the procedure draws the four cards of the mock-up");
    let texts: Vec<String> = documents
        .iter()
        .filter_map(|document| match &document.components[0] {
            Component::Heading { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "Where do you rent?".to_owned(),
            "How big, and where?".to_owned(),
            "Renting the machine".to_owned(),
            "helsinki-1 is ready".to_owned(),
        ]
    );
}

/// The card documents that the bundled skills ship: Set up and the recipes that
/// its steps name, the plan of one task and the move of the server. The two
/// bundled copies of each skill hold the same bytes, and the coverage check keeps
/// them in step, so one copy is read here.
const SKILL_DOCUMENTS: [(&str, &str); 5] = [
    (
        "horizon-setup",
        include_str!("../../../../assets/plugins/claude-code/skills/horizon-setup/SKILL.md"),
    ),
    (
        "horizon-server",
        include_str!("../../../../assets/plugins/claude-code/skills/horizon-server/SKILL.md"),
    ),
    (
        "horizon-machines",
        include_str!("../../../../assets/plugins/claude-code/skills/horizon-machines/SKILL.md"),
    ),
    (
        "horizon-plan",
        include_str!("../../../../assets/plugins/claude-code/skills/horizon-plan/SKILL.md"),
    ),
    (
        "horizon-server-move",
        include_str!("../../../../assets/plugins/claude-code/skills/horizon-server-move/SKILL.md"),
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

/// The longest string of one value of a document, in characters.
fn longest_string(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::String(text) => text.chars().count(),
        serde_json::Value::Array(items) => items.iter().map(longest_string).max().unwrap_or_default(),
        serde_json::Value::Object(fields) => fields.values().map(longest_string).max().unwrap_or_default(),
        _ => 0,
    }
}

/// The longest string of one document, as the 512-character bound counts it.
fn longest_text(document: &BlockDocument) -> usize {
    longest_string(&serde_json::to_value(document).expect("a document of a skill serializes"))
}

#[test]
fn the_card_documents_of_the_set_up_skills_are_valid() {
    let setup = skill_documents(SKILL_DOCUMENTS[0].0, SKILL_DOCUMENTS[0].1);
    let server = skill_documents(SKILL_DOCUMENTS[1].0, SKILL_DOCUMENTS[1].1);
    let machines = skill_documents(SKILL_DOCUMENTS[2].0, SKILL_DOCUMENTS[2].1);
    let plan = skill_documents(SKILL_DOCUMENTS[3].0, SKILL_DOCUMENTS[3].1);
    let server_move = skill_documents(SKILL_DOCUMENTS[4].0, SKILL_DOCUMENTS[4].1);

    assert_eq!(
        setup.len(),
        16,
        "the wizard ships the card of each step, the failure card of a step, and the card of a link that is away"
    );
    assert_eq!(
        machines.len(),
        7,
        "the machine cards and the step list of a rent keep their documents"
    );
    assert!(
        server.is_empty(),
        "the server skill gives commands and no card, so a card cannot drift from its recipe"
    );
    assert_eq!(
        plan.len(),
        7,
        "the plan skill ships the two cards of the short path, the plan of 3, 8 and 12 todos, the card of a stopped plan and the card of an older run"
    );
    assert_eq!(
        server_move.len(),
        4,
        "the move skill ships the plan of the move, the card of the link, the card of the done step and the failure card"
    );
    let headings: Vec<String> = setup
        .iter()
        .filter_map(|document| match &document.components[0] {
            Component::Heading { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        headings,
        vec![
            "What do you want to do?",
            "Where do you rent?",
            "How big, and where?",
            "Renting the machine",
            "helsinki-1 is ready",
            "The machine is ready",
            "Where does the server run?",
            "Setting up the server",
            "The server is ready",
            "The server did not answer",
            "Add another computer",
            "A browser that stays signed in",
            "Preparing the browser",
            "The browser is ready",
            "Join a server that you already have",
            "The step did not finish",
        ],
        "every card of a goal is the card of the mock-up"
    );
    // Every card of a flow takes a press, so the person can move on: a card
    // holds a button or a row that takes one.
    for document in setup.iter().chain(&machines).chain(&plan).chain(&server_move) {
        let takes_a_press = document.components.iter().any(|component| match component {
            Component::Buttons { items } | Component::Callout { buttons: items, .. } => !items.is_empty(),
            Component::List { rows, selection, .. } => {
                *selection == Selection::Single && rows.iter().any(|row| !row.disabled)
            }
            _ => false,
        });
        let heading = match &document.components[0] {
            Component::Heading { text, .. } => text.clone(),
            _ => String::new(),
        };
        assert!(takes_a_press, "the card {heading} leaves the person no press");
    }
}

#[test]
fn every_card_of_a_skill_stays_inside_the_bounds_of_the_host() {
    // Every card of every skill stays inside the bounds that the host applies, so
    // a layout that a person reads cannot be refused at run time. The card of
    // twelve todos with a question and five history lines proves the largest plan.
    let mut bounds = Vec::new();
    for (skill, body) in SKILL_DOCUMENTS {
        for document in skill_documents(skill, body) {
            let heading = document
                .components
                .iter()
                .find_map(|component| match component {
                    Component::Heading { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("{:?}", document.components[0]));
            assert!(
                document.components.len() <= 64,
                "{skill}: the card {heading} holds {} components",
                document.components.len()
            );
            assert!(
                longest_text(&document) <= 512,
                "{skill}: a text of the card {heading} is longer than 512 characters"
            );
            assert!(
                serde_json::to_vec(&document).expect("a document serializes").len() <= 32 * 1024,
                "{skill}: the card {heading} is larger than 32 KiB"
            );
            bounds.push((skill, document));
        }
    }

    let plan: Vec<&BlockDocument> = bounds
        .iter()
        .filter(|(skill, _)| *skill == "horizon-plan")
        .map(|(_, document)| document)
        .collect();
    let boards: Vec<Option<(usize, usize, usize)>> = plan.iter().map(|document| board_counts(document)).collect();
    assert_eq!(
        boards,
        vec![
            Some((1, 1, 1)),
            Some((1, 1, 1)),
            Some((2, 2, 3)),
            Some((5, 4, 8)),
            Some((8, 5, 12)),
            Some((3, 2, 4)),
            None,
        ],
        "the short path draws a board of one wave and one worker, and the plans hold 3, 8 and 12 items"
    );
    let wide: Vec<bool> = plan
        .iter()
        .map(|document| document.width == Some(super::Width::Wide))
        .collect();
    assert_eq!(
        wide,
        vec![false, false, true, true, true, true, false],
        "a plan of three or more todos asks for the wide card, and the short path does not"
    );
    let logs: Vec<Option<usize>> = plan.iter().map(|document| log_entries(document)).collect();
    assert_eq!(
        logs,
        vec![None, Some(1), None, Some(5), Some(5), Some(5), None],
        "a plan that runs shows the last five events of its history and nothing more"
    );
    let answers: Vec<Option<usize>> = plan.iter().map(|document| callout_answers(document)).collect();
    assert_eq!(
        answers,
        vec![None, None, Some(3), Some(3), Some(2), Some(3), None],
        "the callout of the go holds its three answers, and the card of an older run asks nothing"
    );
    assert_eq!(
        plan[4].components.len(),
        7,
        "the card of twelve todos, one question and five history lines holds 7 components of 64"
    );
    assert!(
        plan[2].components.len() <= BlockDocument::MAX_COMPONENTS,
        "the largest plan stays inside the bounds of the host"
    );
    for document in &plan {
        for component in &document.components {
            if let Component::Board { columns, rows, items } = component {
                assert!(columns.len() <= BlockDocument::MAX_BOARD_COLUMNS);
                assert!(rows.len() <= BlockDocument::MAX_BOARD_ROWS);
                assert!(items.len() <= BlockDocument::MAX_BOARD_ITEMS);
                assert!(columns.iter().filter(|column| column.current).count() <= 1);
                let widest = items.iter().fold(0_usize, |widest, item| {
                    let cell = items
                        .iter()
                        .filter(|other| other.column == item.column && other.row == item.row)
                        .count();
                    widest.max(cell)
                });
                assert!(
                    widest <= BlockDocument::MAX_BOARD_CELL_ITEMS,
                    "a cell of a card of a skill holds at most four items: {widest}"
                );
            }
        }
    }
}

/// The waves, the workers and the items of the first board of `document`.
fn board_counts(document: &BlockDocument) -> Option<(usize, usize, usize)> {
    document.components.iter().find_map(|component| match component {
        Component::Board { columns, rows, items } => Some((columns.len(), rows.len(), items.len())),
        _ => None,
    })
}

/// The entries of the first log of `document`.
fn log_entries(document: &BlockDocument) -> Option<usize> {
    document.components.iter().find_map(|component| match component {
        Component::Log { entries } => Some(entries.len()),
        _ => None,
    })
}

/// The answers of the first callout of `document`.
fn callout_answers(document: &BlockDocument) -> Option<usize> {
    document.components.iter().find_map(|component| match component {
        Component::Callout { buttons, .. } => Some(buttons.len()),
        _ => None,
    })
}

/// One wave of a board, named `Wave <index>` with the tag of its index.
fn board_column(index: usize, current: bool) -> serde_json::Value {
    json!({"label": format!("Wave {index}"), "tag": index.to_string(), "value": "~4 m", "current": current})
}

/// One worker of a board.
fn board_row(index: usize) -> serde_json::Value {
    json!({"label": format!("helper-{index} · build-{index}"), "subtitle": "16 vCPU"})
}

fn board_component(columns: usize, rows: usize, items: &[serde_json::Value]) -> String {
    json!({
        "type": "board",
        "columns": (0..columns).map(|index| board_column(index, false)).collect::<Vec<_>>(),
        "rows": (0..rows).map(board_row).collect::<Vec<_>>(),
        "items": items,
    })
    .to_string()
}

fn board_item(column: usize, row: usize) -> serde_json::Value {
    json!({"column": column, "row": row, "title": "Read the issue", "state": "done", "value": "4 m",
           "note": "12 tests pass", "tag": "added"})
}

fn callout_component(buttons: usize) -> String {
    let buttons: Vec<serde_json::Value> = (0..buttons)
        .map(|index| json!({"id": format!("answer.{index}"), "label": "Answer"}))
        .collect();
    json!({"type": "callout", "tone": "warn", "caption": "Needs you · t5", "text": "Which address?",
           "buttons": buttons})
    .to_string()
}

fn log_component(entries: usize) -> String {
    let entries: Vec<serde_json::Value> = (0..entries)
        .map(|index| json!({"at": format!("07:3{index}"), "text": "t3 done on build-2, 9 rows"}))
        .collect();
    json!({"type": "log", "entries": entries}).to_string()
}

#[test]
fn a_board_holds_its_waves_its_workers_and_the_items_of_its_cells() {
    let document = board_component(2, 2, &[board_item(0, 0), board_item(1, 1)]);
    let parsed = parse_document(&document_of(&[document])).expect("a board parses");
    assert_eq!(parsed.width, None, "a document of no width asks for the narrow card");
    let Component::Board { columns, rows, items } = &parsed.components[0] else {
        panic!("the component is a board");
    };
    assert_eq!(columns.len(), 2);
    assert_eq!(rows[1].label, "helper-1 · build-1");
    assert_eq!(items[0].state, super::BoardState::Done);
    assert_eq!(items[1].note.as_deref(), Some("12 tests pass"));
}

#[test]
fn a_board_at_the_limits_parses_and_one_more_wave_row_or_item_is_rejected() {
    let items = |count: usize| {
        (0..count)
            .map(|index| board_item(index % 8, index / 8))
            .collect::<Vec<_>>()
    };
    assert!(
        parse_document(&document_of(&[board_component(
            BlockDocument::MAX_BOARD_COLUMNS,
            BlockDocument::MAX_BOARD_ROWS,
            &items(BlockDocument::MAX_BOARD_ITEMS)
        )]))
        .is_ok()
    );

    for (component, path, actual, limit) in [
        (
            board_component(BlockDocument::MAX_BOARD_COLUMNS + 1, 1, &[]),
            "components[0].columns",
            BlockDocument::MAX_BOARD_COLUMNS + 1,
            BlockDocument::MAX_BOARD_COLUMNS,
        ),
        (
            board_component(1, BlockDocument::MAX_BOARD_ROWS + 1, &[]),
            "components[0].rows",
            BlockDocument::MAX_BOARD_ROWS + 1,
            BlockDocument::MAX_BOARD_ROWS,
        ),
        (
            board_component(8, 8, &items(BlockDocument::MAX_BOARD_ITEMS + 1)),
            "components[0].items",
            BlockDocument::MAX_BOARD_ITEMS + 1,
            BlockDocument::MAX_BOARD_ITEMS,
        ),
    ] {
        let error = parse_document(&document_of(&[component])).expect_err("above the bound");
        assert_eq!(
            error,
            BlockDocumentError::TooManyItems {
                path: path.to_owned(),
                actual,
                limit
            }
        );
    }
}

#[test]
fn a_board_item_names_a_cell_that_its_board_holds() {
    for (item, field, value, limit) in [(board_item(2, 0), "column", 2, 2), (board_item(0, 2), "row", 2, 2)] {
        let error = parse_document(&document_of(&[board_component(2, 2, &[item])])).expect_err("outside");
        assert_eq!(
            error,
            BlockDocumentError::BoardIndexOutOfRange {
                path: format!("components[0].items[0].{field}"),
                value,
                limit,
                name: if field == "column" { "columns" } else { "rows" }
            }
        );
        let message = error.to_string();
        assert!(
            message.contains(field) && message.contains(&limit.to_string()),
            "the refusal names the field and the bound: {message}"
        );
    }
}

#[test]
fn a_fifth_item_of_one_cell_is_rejected_and_four_parse() {
    let four: Vec<serde_json::Value> = (0..BlockDocument::MAX_BOARD_CELL_ITEMS)
        .map(|_| board_item(0, 0))
        .collect();
    assert!(parse_document(&document_of(&[board_component(1, 1, &four)])).is_ok());

    let mut five = four;
    five.push(board_item(0, 0));
    let error = parse_document(&document_of(&[board_component(1, 1, &five)])).expect_err("one over");
    assert_eq!(
        error,
        BlockDocumentError::TooManyCellItems {
            path: "components[0].items[4] (column 0, row 0)".to_owned(),
            actual: BlockDocument::MAX_BOARD_CELL_ITEMS + 1,
            limit: BlockDocument::MAX_BOARD_CELL_ITEMS
        }
    );
}

#[test]
fn a_board_holds_one_current_wave_at_most() {
    let component = json!({
        "type": "board",
        "columns": [board_column(0, false), board_column(1, true), board_column(2, true)],
        "rows": [board_row(1)],
        "items": [],
    })
    .to_string();
    let error = parse_document(&document_of(&[component])).expect_err("two current waves");
    assert_eq!(
        error,
        BlockDocumentError::DuplicateCurrentColumn {
            path: "components[0].columns[2].current".to_owned(),
            first: "components[0].columns[1].current".to_owned()
        }
    );
}

#[test]
fn the_tag_of_a_wave_or_an_item_holds_twelve_characters() {
    let longest = "a".repeat(BlockDocument::MAX_TAG_CHARS);
    let column = json!({"label": "Understand", "tag": longest});
    let item = json!({"column": 0, "row": 0, "title": "Read the issue", "tag": longest});
    let component = json!({"type": "board", "columns": [column], "rows": [board_row(1)], "items": [item]});
    assert!(
        parse_document(&document_of(&[component.to_string()])).is_ok(),
        "twelve characters are accepted"
    );

    let long = "a".repeat(BlockDocument::MAX_TAG_CHARS + 1);
    for (column, item, path) in [
        (
            json!({"label": "Understand", "tag": long}),
            json!({"column": 0, "row": 0, "title": "Read the issue"}),
            "components[0].columns[0].tag",
        ),
        (
            json!({"label": "Understand"}),
            json!({"column": 0, "row": 0, "title": "Read the issue", "tag": long}),
            "components[0].items[0].tag",
        ),
    ] {
        let component = json!({"type": "board", "columns": [column], "rows": [board_row(1)], "items": [item]});
        let error = parse_document(&document_of(&[component.to_string()])).expect_err("one over");
        assert_eq!(
            error,
            BlockDocumentError::TextTooLong {
                path: path.to_owned(),
                actual: BlockDocument::MAX_TAG_CHARS + 1,
                limit: BlockDocument::MAX_TAG_CHARS
            }
        );
    }
}

#[test]
fn a_callout_holds_a_tone_a_caption_and_four_answers() {
    let parsed = parse_document(&document_of(&[callout_component(BlockDocument::MAX_CALLOUT_BUTTONS)]))
        .expect("a callout parses");
    let Component::Callout {
        tone, buttons: items, ..
    } = &parsed.components[0]
    else {
        panic!("the component is a callout");
    };
    assert_eq!(*tone, Tone::Warn);
    assert_eq!(items.len(), BlockDocument::MAX_CALLOUT_BUTTONS);
    assert!(
        parse_document(&document_of(&[callout_component(0)])).is_ok(),
        "no answer is fine"
    );

    let error = parse_document(&document_of(&[callout_component(
        BlockDocument::MAX_CALLOUT_BUTTONS + 1,
    )]))
    .expect_err("one over");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].buttons".to_owned(),
            actual: BlockDocument::MAX_CALLOUT_BUTTONS + 1,
            limit: BlockDocument::MAX_CALLOUT_BUTTONS
        }
    );
}

#[test]
fn the_answers_of_a_callout_share_the_ids_of_the_buttons_of_the_document() {
    let document = json!({
        "components": [
            serde_json::from_str::<serde_json::Value>(&callout_component(1)).expect("a callout"),
            {"type": "buttons", "items": [{"id": "answer.0", "label": "Go"}]},
        ]
    })
    .to_string();
    let error = parse_document(&document).expect_err("the id is taken");
    assert_eq!(
        error,
        BlockDocumentError::DuplicateId {
            path: "components[1].items[0].id".to_owned(),
            id: "answer.0".to_owned()
        }
    );
}

#[test]
fn a_log_holds_sixteen_events_and_no_more() {
    assert!(parse_document(&document_of(&[log_component(BlockDocument::MAX_LOG_ENTRIES)])).is_ok());

    let error =
        parse_document(&document_of(&[log_component(BlockDocument::MAX_LOG_ENTRIES + 1)])).expect_err("one over");
    assert_eq!(
        error,
        BlockDocumentError::TooManyItems {
            path: "components[0].entries".to_owned(),
            actual: BlockDocument::MAX_LOG_ENTRIES + 1,
            limit: BlockDocument::MAX_LOG_ENTRIES
        }
    );
}

#[test]
fn a_document_asks_for_the_narrow_card_the_wide_one_or_neither() {
    let components = log_component(1);
    for (width, expected) in [
        (json!(null), None),
        (json!("narrow"), Some(super::Width::Narrow)),
        (json!("wide"), Some(super::Width::Wide)),
    ] {
        let mut document: serde_json::Value = json!({"components": []});
        let parsed_components: serde_json::Value = serde_json::from_str(&format!("[{components}]")).expect("valid");
        document["components"] = parsed_components;
        if !width.is_null() {
            document["width"] = width.clone();
        }
        let parsed = parse_document(&document.to_string()).expect("the width parses");
        assert_eq!(parsed.width, expected, "{width}");
    }

    let error = parse_document(&format!(r#"{{"width":"huge","components":[{components}]}}"#)).expect_err("refused");
    assert!(matches!(error, BlockDocumentError::InvalidJson { .. }));
}

/// The one-line requests of the plan card procedure are valid `block_panel`
/// requests, and their documents are the cards of the skill. The procedure is an
/// input of this test: a request that drifts from the rules of a card fails here.
#[test]
fn the_requests_of_the_plan_card_procedure_draw_the_cards_of_the_skill() {
    let procedure = include_str!("../../../../docs/testing/procedures/plan-card.md");
    let mut documents = Vec::new();
    for line in procedure.lines() {
        let Some(request) = line.strip_prefix(r#"{"operation":"#) else {
            continue;
        };
        let request: serde_json::Value =
            serde_json::from_str(&format!(r#"{{"operation":{request}"#)).expect("a request is one JSON object");
        let document = request.get("document").expect("a request holds a document");
        let document = parse_document(&document.to_string())
            .unwrap_or_else(|error| panic!("the procedure holds a refused card: {error}"));
        documents.push(document);
    }
    assert_eq!(
        documents.len(),
        9,
        "the procedure replays the two cards of the short path, the plan before the go, the plan with a question, the card of twelve todos, the stopped plan and the three cards of a task that came from an issue"
    );
    let boards: Vec<Option<(usize, usize, usize)>> = documents.iter().map(board_counts).collect();
    assert_eq!(
        boards,
        vec![
            Some((1, 1, 1)),
            Some((1, 1, 1)),
            Some((2, 2, 3)),
            Some((5, 4, 8)),
            Some((8, 5, 12)),
            Some((3, 2, 4)),
            Some((2, 2, 3)),
            Some((4, 2, 4)),
            Some((4, 2, 4)),
        ],
        "each request draws the board of its task, with one wave for the short path"
    );
}
