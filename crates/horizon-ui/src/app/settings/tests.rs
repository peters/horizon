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
