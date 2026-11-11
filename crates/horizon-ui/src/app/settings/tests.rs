use horizon_core::{Config, RuntimeState, StartupDecision};

use super::SettingsStatus;
use crate::app::test_support::test_app_with_startup;

/// The texts that one headless frame of `render` draws, and whether that frame
/// changed a value of the draft.
pub(crate) fn frame_of(mut render: impl FnMut(&mut egui::Ui) -> bool) -> (bool, Vec<String>) {
    use crate::test_egui::DiscardTextures;

    let ctx = egui::Context::default();
    let mut changed = None;
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    changed = Some(render(ui));
                });
            });
        })
        .discard_textures();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }
    (changed.expect("the frame rendered the content"), texts)
}

/// Whether `text` is one of the drawn texts.
pub(crate) fn texts_hold(texts: &[String], text: &str) -> bool {
    texts.iter().any(|drawn| drawn == text)
}

/// Whether one of the drawn texts is `text`, which is what a number and its
/// suffix draw as two texts.
pub(crate) fn at(texts: &[String], text: &str) -> bool {
    texts_hold(texts, text)
}

fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_texts(shape, texts);
            }
        }
        _ => {}
    }
}

#[test]
fn the_general_tab_draws_a_field_for_every_remote_host_setting() {
    let mut config = Config::default();
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));

    assert!(!changed, "a frame that a person did not edit changes nothing");
    for expected in ["Remote hosts", "Workspace", "VNC port", "Remote Sessions"] {
        assert!(
            texts_hold(&texts, expected),
            "the section draws a field for {expected}: {texts:?}"
        );
    }

    // A host with a port of its own is one row, with the button that removes it.
    config.remote_hosts.vnc_ports.insert("smoke-node".to_owned(), 5997);
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));
    assert!(!changed);
    assert!(texts_hold(&texts, "smoke-node"), "{texts:?}");
    assert!(texts_hold(&texts, "Remove"), "{texts:?}");
}

#[test]
fn the_remote_hosts_section_shows_the_rule_of_a_refused_entry() {
    let mut config = Config::default();
    config.remote_hosts.default_workspace = "  ".to_string();
    config.remote_hosts.vnc_ports.insert("  ".to_string(), 5900);
    let (changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));

    assert!(!changed);
    assert!(
        texts
            .iter()
            .any(|text| text.ends_with("remote_hosts.default_workspace cannot be empty")),
        "the rule of the workspace is under its field: {texts:?}"
    );
}

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
        editor.config.appearance.theme = horizon_core::AppearanceTheme::Light;
        editor.dirty = true;
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

/// A settings file that lies beside the store is not a settings source: the
/// editor shows the settings of the store, so a later save cannot put the
/// edited file into the store.
#[test]
fn a_settings_file_beside_the_store_never_reaches_the_editor() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    Config::save(&app.config_path, &app.template_config).expect("write the store");
    std::fs::write(
        &app.config_path,
        "{\n  \"version\": 11,\n  \"appearance\": {\n    \"theme\": \"light\"\n  }\n}\n",
    )
    .expect("write the edited file");

    app.toggle_settings();

    let editor = app.settings.as_ref().expect("open");
    assert_eq!(
        editor.config.appearance.theme,
        horizon_core::AppearanceTheme::Auto,
        "the editor holds the settings of the store, not the file beside it"
    );
}

/// A long value in a field must scroll and must not widen the panel: the panel
/// is a fraction of the viewport, and a wider panel covers the canvas it is
/// meant to sit beside. The widths are compared with the same panel on the same
/// machine, because the default width and the scroll bar differ by platform.
#[test]
fn a_long_text_field_does_not_widen_the_settings_panel() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let viewport = [1600.0, 1000.0];
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.remote_hosts.default_workspace = "a-short-name".to_string();
    }
    let short = widest_panel_width(&ctx, &mut app, viewport);
    if let Some(editor) = app.settings.as_mut() {
        editor.config.remote_hosts.default_workspace = "a-very-long-workspace-name-".repeat(20);
    }
    let long = widest_panel_width(&ctx, &mut app, viewport);

    assert!(
        long <= short + 1.0,
        "a long line must not widen the panel: {short} wide with a short line, {long} with a long line"
    );
    assert!(
        long < viewport[0] / 2.0,
        "the panel stays below half of the 1600 pixel viewport, but it drew {long} wide"
    );
}

/// The widest panel background over three frames, after the panel settles.
fn widest_panel_width(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2]) -> f32 {
    let mut widths = Vec::new();
    for _ in 0..3 {
        let output = crate::app::test_support::run_app_frame_with_input(
            ctx,
            app,
            crate::app::test_support::raw_input(viewport, None),
        );
        widths.push(panel_background_width(&output.shapes));
    }
    widths.into_iter().fold(0.0_f32, f32::max)
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

/// The two machine entries have a field in General, so a render must keep the
/// values that the fields edit: a section that rebuilt its entry would drop a
/// host or a server without a word.
#[test]
fn the_machine_fields_keep_their_entries_while_they_render() {
    let mut config = Config::default();
    config.browser_host.ssh_target = Some("browser-vm".into());
    config.browser_host.profile = Some("mail".into());
    config.server.ssh_target = Some("server-vm".into());
    config.server.command = Some("/opt/horizon --server-link".into());

    let _ = crate::test_egui::DiscardTextures::discard_textures(render_general_tab(&mut config));

    assert_eq!(config.browser_host.ssh_target.as_deref(), Some("browser-vm"));
    assert_eq!(config.browser_host.profile.as_deref(), Some("mail"));
    assert_eq!(config.server.ssh_target.as_deref(), Some("server-vm"));
    assert_eq!(config.server.command.as_deref(), Some("/opt/horizon --server-link"));
}

/// One headless pass of the General tab, with the textures handled so a
/// dropped frame does not report an unapplied delta.
fn render_general_tab(config: &mut Config) -> egui::FullOutput {
    let ctx = egui::Context::default();
    ctx.run_ui(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            super::general::render(ui, config);
        });
    })
}
