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

/// The YAML tab shows long lines, which must scroll and must not widen the
/// panel: the panel is a fraction of the viewport, and a wider panel covers the
/// canvas it is meant to sit beside.
#[test]
fn a_long_yaml_line_does_not_widen_the_settings_panel() {
    use super::{SettingsTab, settings_panel_default_width};

    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let viewport = [1600.0, 1000.0];
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.active_tab = SettingsTab::Yaml;
        editor.buffer = format!(
            "version: 11\nremote_hosts:\n  default_workspace: '{}'\n",
            "a-very-long-workspace-name-".repeat(20)
        );
    }
    let mut widths = Vec::new();
    for _ in 0..3 {
        let output = crate::app::test_support::run_app_frame_with_input(
            &ctx,
            &mut app,
            crate::app::test_support::raw_input(viewport, None),
        );
        widths.push(panel_background_width(&output.shapes));
    }

    let default_width = settings_panel_default_width(viewport[0]);
    let widest = widths.iter().copied().fold(0.0_f32, f32::max);
    assert!(
        widest <= default_width + 1.0,
        "the panel stays at its default width {default_width}, but it drew {widest} wide: {widths:?}"
    );
}

/// The widest painted panel background, which is the Settings panel.
fn panel_background_width(shapes: &[egui::epaint::ClippedShape]) -> f32 {
    let mut widest = 0.0_f32;
    for clipped in shapes {
        visit_width(&clipped.shape, &mut widest);
    }
    widest
}

fn visit_width(shape: &egui::Shape, widest: &mut f32) {
    match shape {
        egui::Shape::Rect(rect) if rect.fill == crate::theme::BG_ELEVATED() => {
            *widest = widest.max(rect.rect.width());
        }
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                visit_width(shape, widest);
            }
        }
        _ => {}
    }
}
