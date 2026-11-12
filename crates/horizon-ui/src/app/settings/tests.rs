use horizon_core::{Config, RuntimeState, StartupDecision};

use super::{SettingsAction, SettingsStatus};
use crate::app::test_support::{run_app_frame, test_app_with_startup};

#[test]
fn a_changed_store_reaches_the_window_through_the_settings_worker() {
    // The frame path of the watch runs under the guard in strict mode: the time
    // of the store and its content are read on the settings worker.
    horizon_core::ui_thread::use_strict_mode();
    let (temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    // The first frame hears the time of the store, which is the baseline of this
    // run and not a change.
    run_app_frame(&ctx, &mut app);
    app.wait_for_settings_io();
    assert!(app.config_mtime_known, "the worker reported the time of the store");

    // A watch within the two seconds of the last one reads nothing.
    app.config_last_check = Some(std::time::Instant::now());
    run_app_frame(&ctx, &mut app);
    assert!(!app.config_probe_pending, "the watch waits for its two seconds");

    // Another window writes the store of this home.
    let changed = Config {
        appearance: horizon_core::AppearanceConfig {
            theme: horizon_core::AppearanceTheme::Light,
        },
        ..app.template_config.clone()
    };
    horizon_core::Config::save(&app.config_path, &changed).expect("the settings of another window");

    app.config_last_check = None;
    run_app_frame(&ctx, &mut app);
    app.wait_for_settings_io();
    app.wait_for_settings_io();
    assert_eq!(
        app.appearance_theme,
        horizon_core::AppearanceTheme::Light,
        "the window applied the settings that the worker read"
    );
    drop(temp);
}

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

pub(super) fn collect_texts(shape: &egui::Shape, texts: &mut Vec<String>) {
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

pub(super) fn ephemeral() -> StartupDecision {
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

/// The Secrets tab draws one row per name, the state of the value on this computer,
/// and the fields that add one. A value that a person typed is never drawn.
#[test]
fn the_secrets_tab_draws_the_names_the_hidden_field_and_the_missing_value() {
    use std::sync::{Arc, Mutex};

    use horizon_core::named_secrets::{NamedSecretVault, SecretName};
    use horizon_core::remote_browser_credential::SharedCredentialStore;
    use horizon_core::secret_store::SecretStore;

    let values = Arc::new(Mutex::new(SecretStore::in_memory().expect("a store in memory")));
    let stored = SecretName::parse("HETZNER_TOKEN").expect("a name");
    values
        .lock()
        .expect("the store")
        .write("named/HETZNER_TOKEN", b"a-value-of-this-computer")
        .expect("the value");
    let vault = Arc::new(NamedSecretVault::new(Box::new(SharedCredentialStore::new(Arc::clone(
        &values,
    )))));
    let mut config = Config::default();
    config.secrets.names = vec!["HETZNER_TOKEN".to_string(), "CLOUD_KEY".to_string()];
    let names = config.secrets.secret_names();
    vault.set_names(config.secrets.names.clone());
    let mut bench = horizon_core::named_secrets::NamedSecretWorkbench::new(Some(vault), &names);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while bench.is_busy() && std::time::Instant::now() < deadline {
        bench.poll();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let mut inputs = super::secrets::SecretInputs::with_typed("NEW_KEY", "TYPED-VALUE-6f1c");
    let (_changed, texts) = frame_of(|ui| super::secrets::render(ui, &mut config, &mut bench, &mut inputs));

    for expected in [
        "HETZNER_TOKEN",
        "CLOUD_KEY",
        "Secrets for agents",
        "Replace value",
        "Delete",
        "Add",
        "2 of 64 names",
    ] {
        assert!(texts_hold(&texts, expected), "the tab draws {expected}: {texts:?}");
    }
    assert!(
        texts.iter().any(|text| text.contains("no value on this computer")),
        "a name whose value this computer does not hold says so: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("credential store")),
        "a named secret was never in the keyring of the system, so the tab says nothing about it: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("TYPED-VALUE-6f1c")),
        "a typed value is never drawn: {texts:?}"
    );
    assert_eq!(
        inputs.typed(),
        ("NEW_KEY".to_string(), "TYPED-VALUE-6f1c".to_string()),
        "a frame that drew the fields keeps the draft of the person"
    );
    assert!(matches!(
        bench.state(&stored),
        horizon_core::named_secrets::NamedSecretState::Present
    ));
}

/// A window with its Settings window open, and a draft that the person edits.
fn editor_app() -> (tempfile::TempDir, crate::app::HorizonApp) {
    let (temp, mut app) = crate::app::test_support::test_app();
    app.toggle_settings();
    assert!(app.settings.is_some(), "the Settings window is open");
    (temp, app)
}

#[test]
fn a_value_of_another_computer_keeps_the_edits_of_a_section_and_applies_the_others() {
    let (_temp, mut app) = editor_app();
    let stored = app.template_config.clone();
    // The person edits the theme and leaves the minimap at the value of the store.
    if let Some(editor) = app.settings.as_mut() {
        editor.config.appearance.theme = horizon_core::AppearanceTheme::Light;
        editor.dirty = true;
        editor.status = SettingsStatus::LivePreview;
    }

    // Another computer changed both sections in the store.
    let mut other = stored.clone();
    other.appearance.theme = horizon_core::AppearanceTheme::Dark;
    other.overlays.minimap_width = 500.0;
    horizon_core::Config::save(&app.config_path, &other).expect("the settings of another computer");
    app.adopt_reloaded_settings(&other);

    let editor = app.settings.as_ref().expect("the Settings window");
    assert_eq!(
        editor.config.appearance.theme,
        horizon_core::AppearanceTheme::Light,
        "the draft keeps the edit of the person"
    );
    assert_eq!(
        editor.config.overlays.minimap_width, 500.0,
        "a section that the person does not edit applies at once"
    );
    let conflict = editor.conflict.as_ref().expect("one line about the other computer");
    assert_eq!(conflict.sections, vec!["appearance".to_owned()]);
    assert_eq!(
        conflict.stored.appearance.theme,
        horizon_core::AppearanceTheme::Dark,
        "the window holds the value that it did not take"
    );
    assert_eq!(
        app.appearance_theme,
        horizon_core::AppearanceTheme::Light,
        "the window shows the draft of the person"
    );

    // Keep editing: the draft stays and the line goes.
    app.apply_settings_action(SettingsAction::KeepEditing);
    let editor = app.settings.as_ref().expect("the Settings window");
    assert!(editor.conflict.is_none());
    assert_eq!(editor.config.appearance.theme, horizon_core::AppearanceTheme::Light);
}

#[test]
fn the_value_of_another_computer_is_taken_when_the_person_takes_it() {
    let (_temp, mut app) = editor_app();
    let stored = app.template_config.clone();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.appearance.theme = horizon_core::AppearanceTheme::Light;
        editor.dirty = true;
    }
    let mut other = stored.clone();
    other.appearance.theme = horizon_core::AppearanceTheme::Dark;
    horizon_core::Config::save(&app.config_path, &other).expect("the settings of another computer");
    app.adopt_reloaded_settings(&other);
    assert!(
        app.settings.as_ref().is_some_and(|editor| editor.conflict.is_some()),
        "the window offers the value"
    );

    app.apply_settings_action(SettingsAction::TakeRemote);
    let editor = app.settings.as_ref().expect("the Settings window");
    assert!(editor.conflict.is_none());
    assert_eq!(
        editor.config.appearance.theme,
        horizon_core::AppearanceTheme::Dark,
        "the draft takes the value of the other computer"
    );
    assert!(!editor.dirty, "the draft is the value of the store again");
    assert_eq!(app.appearance_theme, horizon_core::AppearanceTheme::Dark);
}

/// A theme that the person picks in the tab is drawn at once, in the frame that
/// applies it: the panel of that frame takes the palette of the new theme before it
/// paints anything, so its fields, check boxes and text boxes must take the same
/// theme in that frame. A frame that changes the style of the context but not the
/// style of its own ui leaves a dark panel around light fields until the next
/// frame, which an idle window reaches only at its next refresh.
#[test]
fn the_frame_that_applies_another_theme_draws_its_fields_with_it() {
    use horizon_core::{AppearanceTheme, Config, RuntimeState, StartupDecision};

    let mut config = Config::default();
    config.appearance.theme = AppearanceTheme::Light;
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    app.toggle_settings();
    assert!(
        !crate::app::test_support::run_app_frame_visuals(&ctx, &mut app).dark_mode,
        "the window draws the light theme of its store"
    );

    // The person picks Dark in the General tab, which previews the theme at once.
    let mut draft = app.template_config.clone();
    draft.appearance.theme = AppearanceTheme::Dark;
    app.apply_live_preview(&draft);
    assert!(!app.theme_applied, "the frame after the pick applies the theme");

    let visuals = crate::app::test_support::run_app_frame_visuals(&ctx, &mut app);
    assert_eq!(app.resolved_theme, crate::theme::ResolvedTheme::Dark);
    assert!(app.theme_applied, "the theme of the pick is applied");
    assert!(
        visuals.dark_mode,
        "the fields of the frame that applies the theme are drawn in it"
    );
}
