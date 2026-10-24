use horizon_core::{Config, RuntimeState, StartupDecision};

use super::SettingsStatus;
use crate::app::test_support::test_app_with_startup;

fn ephemeral() -> StartupDecision {
    StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    }
}

#[test]
fn a_save_shows_saving_and_then_saved() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.buffer = Config::default().to_yaml().expect("yaml");
        editor.buffer = editor.buffer.replace("theme: auto", "theme: light");
    }

    app.save_settings();
    assert!(
        matches!(app.settings.as_ref().expect("open").status, SettingsStatus::Saving),
        "the status line says that the write runs"
    );

    app.wait_for_settings_io();
    let editor = app.settings.as_ref().expect("open");
    assert!(
        matches!(editor.status, SettingsStatus::Saved),
        "the status line says that the write finished"
    );
    assert_eq!(
        app.template_config.appearance.theme,
        horizon_core::AppearanceTheme::Light
    );
}

/// A YAML copy that a person exported and then edited is not a settings source:
/// the editor shows the settings of this run, so a later save cannot put the
/// edited file into the store.
#[test]
fn an_edited_export_never_reaches_the_editor() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    let mut exported = app.template_config.to_yaml().expect("yaml");
    exported = exported.replace("theme: auto", "theme: light");
    std::fs::write(&app.config_path, &exported).expect("write the edited export");

    app.toggle_settings();

    let editor = app.settings.as_ref().expect("open");
    assert!(
        !editor.buffer.contains("theme: light"),
        "the editor holds the settings of this run, not the edited export: {}",
        editor.buffer
    );
    assert!(editor.buffer.contains("theme: auto"), "{}", editor.buffer);
}
