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
fn the_tab_row_fits_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    let labels: Vec<&str> = super::SettingsTab::ALL.iter().map(|tab| tab.label()).collect();
    let ctx = egui::Context::default();
    let mut needed = 0.0;
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            needed = ui.ctx().fonts_mut(|fonts| {
                super::tab_row_width(
                    |label| {
                        fonts
                            .layout_no_wrap(
                                label.to_owned(),
                                egui::FontId::proportional(super::TAB_LABEL_SIZE),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x
                    },
                    &labels,
                )
            });
        })
        .discard_textures();

    // The default window of this build, so the panel has its default width.
    let panel = super::settings_panel_default_width(1600.0);
    let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
    assert!(
        needed <= content,
        "the {labels:?} tabs need {needed} and the panel gives {content}"
    );
}

#[test]
fn every_tab_draws_on_one_line_at_the_default_panel_width() {
    use crate::test_egui::DiscardTextures;

    let panel = super::settings_panel_default_width(1600.0);
    let content = panel - 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X);
    let mut editor = super::SettingsEditor {
        config: Config::default(),
        original: Config::default(),
        dirty: false,
        status: super::SettingsStatus::None,
        active_tab: super::SettingsTab::General,
        credential_inputs: super::remote_browsers::CredentialInputs::default(),
        portable_profile: super::remote_browsers::PortableProfilePanel::new(std::path::Path::new("/s.json")),
        provider_usage: super::remote_usage::UsagePanels::default(),
        secret_inputs: super::secrets::SecretInputs::default(),
    };
    let ctx = egui::Context::default();
    let mut measured = (0.0_f32, 0.0_f32);
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(content + 2.0 * f32::from(super::SETTINGS_PANEL_MARGIN_X), 600.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                ui.set_width(content);
                super::render_tab_bar(ui, &mut editor);
                measured = (ui.min_rect().height(), content);
            },
        )
        .discard_textures();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect_texts(&clipped.shape, &mut texts);
    }

    // One row of a 12 px label with 5 px of vertical padding is under 30 points; a
    // second row would be above 40.
    assert!(
        measured.0 <= 34.0,
        "the tabs stay on one line in {} points: {texts:?}",
        measured.1
    );
    for tab in ["General", "Shortcuts", "Presets", "Browser", "Remote", "Secrets"] {
        assert!(texts_hold(&texts, tab), "the row draws {tab}: {texts:?}");
    }
}

#[test]
fn a_port_row_gives_the_host_box_a_useful_width_and_a_hint() {
    let mut config = Config::default();
    let (_changed, texts) = frame_of(|ui| super::general::render(ui, &mut config));
    assert!(
        texts_hold(&texts, "Host label or SSH host name"),
        "the empty host box says what belongs in it: {texts:?}"
    );

    // The box keeps the room that its row needs: it is never narrower than a host
    // name and never wide enough to strand the port far from the host.
    assert!((super::general::host_field_width(0.0) - 140.0).abs() < f32::EPSILON);
    assert!((super::general::host_field_width(432.0) - 260.0).abs() < f32::EPSILON);
    assert!((super::general::host_field_width(300.0) - 146.0).abs() < f32::EPSILON);
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

/// The width of the Settings panel on each tab, with a long path in the Remote
/// tab and a long secret name in the Secrets tab. The panel is one panel beside
/// the canvas, so its rectangle must not change when the person changes the tab:
/// a wider panel covers the panels that it sits beside.
#[test]
fn the_settings_panel_keeps_one_width_on_every_tab() {
    use std::collections::BTreeMap;

    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
    };

    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let viewport = [1800.0, 1000.0];
    app.toggle_settings();
    let long_path = "/home/user/project/".to_string() + &"a-folder-name-that-is-long/".repeat(8);
    let long_reference = "a-reference-name-of-a-credential-that-is-long".to_string();
    let reference = CredentialReference::from(long_reference.as_str());
    let mut bindings = BTreeMap::new();
    bindings.insert(
        reference.clone(),
        CredentialBinding {
            store: CredentialStoreKind::Session,
            slot: None,
        },
    );
    let mut providers = BTreeMap::new();
    providers.insert(
        "a-provider-with-a-long-name".to_string(),
        RemoteProviderProfile {
            adapter: RemoteAdapterKind::Webdriver,
            endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("the endpoint"),
            authentication: RemoteAuthentication::Bearer { token_ref: reference },
            credential_bindings: bindings,
            limits: RemoteSessionLimits::default(),
        },
    );
    if let Some(editor) = app.settings.as_mut() {
        editor.portable_profile = super::remote_browsers::PortableProfilePanel::new(std::path::Path::new(&format!(
            "{long_path}profile.json"
        )));
        editor.config.browser.remote.providers = providers;
        editor.config.secrets.names = vec!["HETZNER_API_TOKEN_FOR_THE_BUILD_MACHINE_OF_THE_LAB".to_string()];
    }

    let mut widths: Vec<(String, f32)> = Vec::new();
    for tab in [
        super::SettingsTab::General,
        super::SettingsTab::RemoteBrowsers,
        super::SettingsTab::Secrets,
    ] {
        if let Some(editor) = app.settings.as_mut() {
            editor.active_tab = tab;
        }
        widths.push((tab.label().to_string(), widest_panel_width(&ctx, &mut app, viewport)));
    }

    let (first_tab, first) = widths.first().expect("one width");
    for (tab, width) in &widths {
        assert!(
            (width - first).abs() <= 1.0,
            "the panel is {first} wide on {first_tab} and {width} wide on {tab}: {widths:?}"
        );
    }
    assert!(
        *first < viewport[0] / 2.0,
        "the panel stays below half of the viewport: {widths:?}"
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

/// The width of the Settings panel, which is the width that the panel takes from
/// the window. The background of the panel is the only shape of that colour, and
/// the panel cuts its own painting at its rectangle, so the clip rectangle of that
/// shape is the panel.
fn panel_background_width(shapes: &[egui::epaint::ClippedShape]) -> f32 {
    let mut widest = 0.0_f32;
    for clipped in shapes {
        if holds_background(&clipped.shape) {
            widest = widest.max(clipped.clip_rect.width());
        }
    }
    widest
}

fn holds_background(shape: &egui::Shape) -> bool {
    match shape {
        egui::Shape::Rect(rect) => rect.fill == crate::theme::BG_ELEVATED(),
        egui::Shape::Vec(shapes) => shapes.iter().any(holds_background),
        _ => false,
    }
}
