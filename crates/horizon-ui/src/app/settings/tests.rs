use horizon_core::{AppearanceTheme, Config, RuntimeState, StartupDecision};

use super::{SettingsAction, SettingsStatus};
use crate::app::test_support::{
    browser_panel_options, raw_input, run_app_frame, run_app_frame_with_input, test_app_with_startup,
};

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
    let (changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));

    assert!(!changed, "a frame that a person did not edit changes nothing");
    for expected in ["Remote hosts", "Workspace", "VNC port", "Remote Sessions"] {
        assert!(
            texts_hold(&texts, expected),
            "the section draws a field for {expected}: {texts:?}"
        );
    }

    // A host with a port of its own is one row, with the button that removes it.
    config.remote_hosts.vnc_ports.insert("smoke-node".to_owned(), 5997);
    let (changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));
    assert!(!changed);
    assert!(texts_hold(&texts, "smoke-node"), "{texts:?}");
    assert!(texts_hold(&texts, "Remove"), "{texts:?}");
}

#[test]
fn a_port_row_gives_the_host_box_a_useful_width_and_a_hint() {
    let mut config = Config::default();
    let (_changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));
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
    let (changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));

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
            super::general::render(ui, config, super::general::GeneralView::default());
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

#[test]
fn the_general_tab_draws_the_three_choices_of_a_desktop_notification() {
    use horizon_core::NotificationSetting;

    let mut config = Config::default();
    let (_changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));
    assert!(
        texts_hold(&texts, "Desktop notifications"),
        "the row names the setting: {texts:?}"
    );
    for setting in NotificationSetting::VALUES {
        assert!(
            texts_hold(&texts, setting.label()),
            "the row offers {setting:?}: {texts:?}"
        );
    }
    assert!(
        texts
            .iter()
            .any(|drawn| drawn.contains("while no Horizon window has the focus")),
        "the help says when a notification comes: {texts:?}"
    );
}

#[test]
fn the_row_of_desktop_notifications_says_when_this_computer_cannot_show_one() {
    let mut config = Config::default();
    let view = super::general::GeneralView {
        missing_notification_program: Some("notify-send is not installed"),
    };
    let (_changed, texts) = frame_of(|ui| super::general::render(ui, &mut config, view));
    assert!(
        texts.iter().any(|text| text.contains("notify-send is not installed")),
        "the row says that the program is absent: {texts:?}"
    );

    // A computer that has the program draws no such line.
    let (_changed, texts) =
        frame_of(|ui| super::general::render(ui, &mut config, super::general::GeneralView::default()));
    assert!(!texts.iter().any(|text| text.contains("is not installed")), "{texts:?}");
}

#[test]
fn a_chosen_desktop_notification_reaches_the_draft() {
    use horizon_core::NotificationSetting;

    let mut config = Config::default();
    assert_eq!(
        config.features.notifications,
        NotificationSetting::NeedsYouAndDone,
        "a fresh configuration speaks of both changes"
    );
    // The value is part of the section that a computer sends to its server, so a
    // save of the draft carries it.
    config.features.notifications = NotificationSetting::Off;
    let document = config.to_json().expect("the settings of the draft");
    assert!(document.contains("\"notifications\": \"off\""), "{document}");
    let read = Config::from_json(&document).expect("the settings of the draft");
    assert_eq!(read.features.notifications, NotificationSetting::Off);
}

/// A save of the browser host of the Settings window serves the very next
/// browser panel of this run, and it needs no start.
#[test]
fn a_save_of_the_browser_host_serves_the_next_browser_panel() {
    let (_temp, _ctx, mut app) = test_app_with_startup(ephemeral());
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.browser_host.ssh_target = Some("browser-vm".to_owned());
        editor.config.browser_host.profile = Some("work".to_owned());
        editor.dirty = true;
    }

    app.save_settings();
    app.wait_for_settings_io();

    let workspace = app.board.ensure_workspace();
    let panel = app
        .create_panel_with_options_beside(browser_panel_options(&app, true), workspace, None)
        .expect("a browser panel");
    let browser = app
        .board
        .panel(panel)
        .and_then(|panel| panel.browser())
        .expect("a browser panel");
    assert_eq!(
        browser.attached_host_target(),
        Some("browser-vm"),
        "the panel that the person opened after the save uses the host of the save"
    );
    assert_eq!(browser.attached_host_profile(), Some("work"));
}

/// One terminal panel of a shell that printed many lines, under the Settings pane.
fn app_with_a_terminal_under_the_pane() -> (
    tempfile::TempDir,
    egui::Context,
    crate::app::HorizonApp,
    horizon_core::PanelId,
) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.initial_pan_done = true;
    app.root_viewport_stabilizer = None;
    let workspace = app.board.ensure_workspace();
    let panel = app
        .create_panel_with_options(
            horizon_core::PanelOptions {
                kind: horizon_core::PanelKind::Command,
                command: Some("/bin/sh".to_owned()),
                args: vec![
                    "-c".to_owned(),
                    "i=0; while [ $i -lt 200 ]; do printf 'LINE-%s\\n' \"$i\"; i=$((i + 1)); done; sleep 60".to_owned(),
                ],
                position: Some([560.0, 60.0]),
                size: Some([560.0, 360.0]),
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .expect("the terminal of the test");
    app.settle_panel_starts();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.drain_panel_output();
        let text = app
            .board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .map_or_else(String::new, |terminal| terminal.last_lines_text(20));
        if text.contains("LINE-199") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the terminal of the test prints its lines"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    app.toggle_settings();
    for _ in 0..3 {
        let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([1200.0, 600.0], None));
    }
    (temp, ctx, app, panel)
}

/// One frame of the window with the events of the person.
fn window_frame(ctx: &egui::Context, app: &mut crate::app::HorizonApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut input = raw_input([1200.0, 600.0], None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input)
}

/// The rectangle that the text `wanted` covers in one frame of the window.
fn window_text(output: &egui::FullOutput, wanted: &str) -> Option<egui::Rect> {
    output.shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::epaint::Shape::Text(text) if text.galley.job.text == wanted => {
            Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
        }
        _ => None,
    })
}

/// A press and its release at `pos` in the events of one frame: a display that
/// reports slowly delivers both before the next frame draws.
fn press_and_release(pos: egui::Pos2) -> Vec<egui::Event> {
    let mut events = vec![egui::Event::PointerMoved(pos)];
    for pressed in [true, false] {
        events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    events
}

/// The y of the text `wanted` in one frame of the window.
fn window_text_y(ctx: &egui::Context, app: &mut crate::app::HorizonApp, wanted: &str) -> Option<f32> {
    window_text(&window_frame(ctx, app, Vec::new()), wanted).map(|rect| rect.min.y)
}

#[test]
fn the_wheel_over_the_settings_pane_scrolls_the_settings_and_not_the_terminal_behind() {
    let (_temp, ctx, mut app, panel) = app_with_a_terminal_under_the_pane();
    let pane = app
        .settings
        .as_ref()
        .and_then(|editor| editor.panel_rect)
        .expect("the rectangle of the pane");
    let pointer = pane.center();
    let scrollback = app
        .board
        .panel(panel)
        .and_then(horizon_core::Panel::terminal)
        .expect("the terminal of the test")
        .scrollback();
    let heading = window_text_y(&ctx, &mut app, "Appearance").expect("the first section of the tab");

    let mut clicked = raw_input([1200.0, 600.0], None);
    clicked.events = vec![
        egui::Event::PointerMoved(pointer),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            source: egui::MouseWheelSource::Unknown,
            delta: egui::vec2(0.0, -3.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let _ = run_app_frame_with_input(&ctx, &mut app, clicked);
    let mut moved = None;
    for _ in 0..6 {
        moved = window_text_y(&ctx, &mut app, "Appearance");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    assert_eq!(
        app.board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .expect("the terminal of the test")
            .scrollback(),
        scrollback,
        "the terminal behind the pane receives no wheel"
    );
    assert!(
        moved.is_some_and(|moved| moved < heading),
        "the pane scrolls: the section was at {heading} and is at {moved:?}"
    );
}

/// The theme drop-down of the General tab is a combo box: its list is a pop-up
/// that the pane opens over itself. The pointer must belong to that list where
/// the list is painted, and a press on its entry must reach the list.
#[test]
fn the_list_of_the_theme_drop_down_opens_above_the_pane() {
    let (_temp, ctx, mut app, _panel) = app_with_a_terminal_under_the_pane();
    let button = window_text(&window_frame(&ctx, &mut app, Vec::new()), "Auto (system)")
        .expect("the theme drop-down shows the theme of the draft");
    let _ = window_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(button.center())]);
    let pane_layer = ctx
        .layer_id_at(button.center())
        .expect("the layer of the pane holds the drop-down");
    // A slow display delivers the press and its release in one frame.
    let _ = window_frame(&ctx, &mut app, press_and_release(button.center()));
    // The first frame of a pop-up sizes it; the list of entries follows it.
    let mut opened = None;
    for _ in 0..3 {
        opened = window_text(&window_frame(&ctx, &mut app, Vec::new()), "Light");
    }
    let entry = opened.expect("the list of the drop-down opened");
    let entry_layer = ctx
        .layer_id_at(entry.center())
        .expect("a layer holds the entry of the list");
    assert_ne!(
        entry_layer, pane_layer,
        "the pointer over the entry belongs to the list and not to the pane"
    );
    assert_eq!(
        entry_layer.order,
        egui::Order::Foreground,
        "the list is the pop-up layer above the pane"
    );

    let _ = window_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(entry.center())]);
    let _ = window_frame(&ctx, &mut app, press_and_release(entry.center()));
    let _ = window_frame(&ctx, &mut app, Vec::new());
    assert_eq!(
        app.settings.as_ref().expect("the pane is open").config.appearance.theme,
        AppearanceTheme::Light,
        "the press on the entry of the list takes the theme of the draft"
    );
}

/// The rectangle that the pane takes of the window.
fn pane_rect(app: &crate::app::HorizonApp) -> egui::Rect {
    app.settings
        .as_ref()
        .and_then(|editor| editor.panel_rect)
        .expect("the rectangle of the pane")
}

/// The rectangle of the text `wanted` after the pane scrolled far enough to show
/// its centre, or `None` while the tab has nothing more to show.
fn window_text_scrolled_to(ctx: &egui::Context, app: &mut crate::app::HorizonApp, wanted: &str) -> Option<egui::Rect> {
    let pane = pane_rect(app);
    for _ in 0..8 {
        if let Some(rect) = window_text(&window_frame(ctx, app, Vec::new()), wanted)
            && pane.contains(rect.center())
        {
            return Some(rect);
        }
        let mut input = raw_input([1200.0, 600.0], None);
        input.events = vec![
            egui::Event::PointerMoved(pane.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                source: egui::MouseWheelSource::Unknown,
                delta: egui::vec2(0.0, -3.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let _ = run_app_frame_with_input(ctx, app, input);
    }
    None
}

/// A press on a check box of the pane changes its value, and the terminal that
/// its panel draws behind the pane receives nothing; the canvas clips that panel
/// at its edge, so only the layer of the pane can take the press.
#[test]
fn a_press_on_a_check_box_of_the_pane_reaches_no_terminal_behind_it() {
    let (_temp, ctx, mut app, _panel) = app_with_a_terminal_under_the_pane();
    app.board.focused = None;
    let check_box = window_text_scrolled_to(&ctx, &mut app, "Organize Workspaces on Session Load")
        .expect("the pane scrolled to the check box of the test");
    let before = app
        .settings
        .as_ref()
        .expect("the pane is open")
        .config
        .features
        .organize_workspaces_on_session_load;

    // The hit test of the next frame reads the widgets of this one, so the press
    // lands on the box that the frame above found.
    let _ = window_frame(&ctx, &mut app, press_and_release(check_box.center()));
    let _ = window_frame(&ctx, &mut app, Vec::new());

    assert_ne!(
        app.settings
            .as_ref()
            .expect("the pane is open")
            .config
            .features
            .organize_workspaces_on_session_load,
        before,
        "the press on the check box of the pane changed it"
    );
    assert_eq!(
        app.board.focused, None,
        "the terminal behind the pane received no press, so no panel took the focus"
    );
}

/// A drag that starts on the title bar of a panel and passes over the pane keeps
/// moving that panel until the release: the pane takes the pointer only while no
/// pointer button is held.
#[test]
fn a_drag_of_a_panel_over_the_pane_keeps_moving_the_panel_until_the_release() {
    let (_temp, ctx, mut app, panel) = app_with_a_terminal_under_the_pane();
    let pane = pane_rect(&app);
    // The canvas shows only what the pane leaves of it, so the panel keeps its
    // room there while the drag moves it to the right.
    app.board.move_panel(panel, [100.0, 60.0]);
    let _ = window_frame(&ctx, &mut app, Vec::new());
    let screen = *app.panel_screen_rects.get(&panel).expect("the panel is on screen");
    let start = egui::Pos2::new(screen.max.x - 10.0, screen.min.y + 17.0);
    assert!(!pane.contains(start), "the drag starts beside the pane");
    let position = |app: &crate::app::HorizonApp| {
        let panel = app.board.panel(panel).expect("the panel of the test");
        egui::Pos2::new(panel.layout.position[0], panel.layout.position[1])
    };
    let before = position(&app);
    let _ = window_frame(
        &ctx,
        &mut app,
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let mut held = position(&app);
    for step in [100.0, 200.0] {
        let point = start + egui::vec2(step, 0.0);
        assert!(pane.contains(point), "the drag passes over the pane");
        let _ = window_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(point)]);
        let moved = position(&app);
        assert!(
            moved.x > held.x + 90.0,
            "the panel follows the pointer over the pane: {held:?} -> {moved:?}"
        );
        held = moved;
    }
    let end = start + egui::vec2(200.0, 0.0);
    let _ = window_frame(
        &ctx,
        &mut app,
        vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let released = position(&app);
    assert_eq!(released, held, "the release keeps the panel where the drag left it");
    assert!(released.x > before.x + 150.0, "the drag moved the panel over the pane");
}
