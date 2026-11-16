//! The document of a card as a caller sends it: the object itself, the JSON text
//! of it, and the bounds that both forms pass.

use super::*;

/// The document as an agent program sends it when it builds every tool argument
/// as text: the JSON text of the document.
fn document_text(heading: &str) -> serde_json::Value {
    serde_json::Value::String(serde_json::to_string(&document(heading)).expect("the text of a document"))
}

fn drawn(app: &HorizonApp, panel_id: &str) -> BlockDocument {
    app.board
        .panel(app.board.panel_id_by_local_id(panel_id).expect("card"))
        .and_then(Panel::block)
        .expect("card state")
        .document
        .clone()
}

fn too_large_document() -> serde_json::Value {
    serde_json::json!({
        "components": [{"type": "text", "text": "x".repeat(horizon_browser_control::block::BlockDocument::MAX_SERIALIZED_BYTES)}]
    })
}

fn create_text(app: &mut HorizonApp, ctx: &Context, text: &str) -> Outcome {
    app.apply_block_request(
        &request(
            app,
            Operation::Create {
                title: None,
                document: serde_json::Value::String(text.to_owned()),
            },
        ),
        ctx,
    )
}

#[test]
fn a_document_that_arrives_as_the_text_of_the_document_draws_the_same_card() {
    let (_temp, ctx, mut app) = app();
    let (object_card, _) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document("note"),
            },
        ),
        &ctx,
    ));
    let (text_card, revision) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document_text("note"),
            },
        ),
        &ctx,
    ));
    assert_eq!(revision, 1);
    assert_eq!(
        drawn(&app, &text_card),
        drawn(&app, &object_card),
        "the text of a document draws the card of the document itself"
    );

    let updated = app.apply_block_request(
        &request(
            &app,
            Operation::Update {
                panel_id: text_card.clone(),
                document: document_text("second"),
            },
        ),
        &ctx,
    );
    assert!(matches!(updated, Outcome::Updated { revision: 2, .. }), "{updated:?}");
    assert_eq!(
        drawn(&app, &text_card).components,
        card_document(&document("second")).expect("document").components
    );
}

#[test]
fn a_text_that_is_no_document_is_still_an_invalid_document() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    for text in [
        "not a document",
        "[]",
        r#"{"components": [{"type": "heading"}]}"#,
        r#""{\"components\": []}""#,
    ] {
        let refusal = create_text(&mut app, &ctx, text);
        assert_eq!(code(&refusal), "invalid_document", "{text}");
    }
    assert_eq!(app.board.panels.len(), before, "a refused text creates no card");
}

#[test]
fn a_document_that_breaks_a_rule_names_the_offending_place() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    let invalid = serde_json::json!({
        "components": [{"type": "buttons", "items": [{"id": "Not An Id", "label": "Stop"}]}]
    });
    let refusal = app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: invalid,
            },
        ),
        &ctx,
    );
    let Outcome::Failed { code: failure, message } = &refusal else {
        panic!("expected a refusal: {refusal:?}");
    };
    assert_eq!(failure, "invalid_document");
    assert!(message.contains("components[0].items[0].id"), "{message}");
    assert_eq!(app.board.panels.len(), before, "a refused document creates no card");

    let too_large = app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: too_large_document(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&too_large), "document_too_large");

    let text = serde_json::to_string(&too_large_document()).expect("a text");
    assert_eq!(
        code(&create_text(&mut app, &ctx, &text)),
        "document_too_large",
        "the size bound counts the text that the caller sent"
    );
}
