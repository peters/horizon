//! The name of this computer on the live link. The label of the link is the name
//! that the settings store holds: a draft of the Settings window is no name that
//! another computer reads, so a key in the name field sends nothing.

use horizon_core::{RuntimeState, StartupDecision};

use super::super::{link_renamed, stored_server};
use crate::app::HorizonApp;
use crate::app::test_support::{run_app_frame, test_app_with_startup};

/// The name that the live link of `app` reads, and whether it asks to start again
/// for a new one. The label of the link and its restart read this one value.
fn link_name(app: &HorizonApp) -> (&str, bool) {
    let server = stored_server(
        app.settings.as_ref().map(|editor| &editor.original),
        &app.template_config,
    );
    (
        server.computer_name.as_str(),
        link_renamed(&app.shared_terminals, server),
    )
}

/// A window whose store holds `name`, with the live link started under it and the
/// Settings window open.
fn window_with_the_saved_name(name: &str) -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.template_config.server.computer_name = name.to_owned();
    app.shared_terminals.name = name.to_owned();
    app.toggle_settings();
    run_app_frame(&ctx, &mut app);
    if let Some(editor) = app.settings.as_mut() {
        editor.original.server.computer_name = name.to_owned();
        editor.config.server.computer_name = name.to_owned();
    }
    (temp, ctx, app)
}

/// Typing in `Name of this computer` applies the draft before Save, as every
/// setting of the window does. The link keeps the name that the store saved, so
/// the other computers see its list go and come once, at the Save.
#[test]
fn a_draft_of_the_name_starts_no_link_and_a_save_hands_the_new_name_over_once() {
    let (_temp, ctx, mut app) = window_with_the_saved_name("desk-a");
    assert_eq!(
        link_name(&app),
        ("desk-a", false),
        "the link started with the saved name"
    );

    // The person types "desk-b": the window holds the draft and applies it to the
    // settings of this process before Save.
    let draft = {
        let editor = app.settings.as_mut().expect("the settings window of a test");
        editor.config.server.computer_name = "desk-b".to_owned();
        editor.config.clone()
    };
    app.template_config = draft.clone();
    run_app_frame(&ctx, &mut app);
    assert_eq!(
        app.template_config.server.computer_name, "desk-b",
        "the draft is applied"
    );
    assert_eq!(
        link_name(&app),
        ("desk-a", false),
        "a draft of the name sends nothing and starts nothing on the link"
    );

    // Save writes the draft into the store, and the reply of the write is the
    // baseline that the link reads.
    app.settings
        .as_mut()
        .expect("the settings window of a test")
        .adopt_saved(draft);
    assert_eq!(
        link_name(&app),
        ("desk-b", true),
        "the saved name is the label of the link, and it asks for one start"
    );

    // The start takes the new name, so the link asks for no other one.
    app.shared_terminals.name = "desk-b".to_owned();
    assert_eq!(link_name(&app), ("desk-b", false), "one start takes the new name");
}
