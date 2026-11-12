//! The single-line fields of the Settings panel take the keys of a person,
//! spaces included, one frame after another.
//!
//! A field that feeds its own trimmed draft back takes the space that ends a word
//! away before the next key of that word arrives, so a person cannot type the space
//! of `docker exec` in a field of this panel. These tests press the keys through the
//! widgets with the events that a window sends, one key per frame, as a person types.

use egui::{Event, Key, Modifiers, Pos2};

use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_config_and_startup};
use horizon_core::{Config, PanelKind, PanelResume, PresetConfig, RuntimeState, StartupDecision};

/// The size of the window of a test, which is the smallest that this build opens
/// wide enough that the whole panel of a tab is on it.
const WINDOW: [f32; 2] = [1600.0, 1000.0];

fn key(key: Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// The press and the text of one key that a person types, as a window reports them.
fn typed(key_code: Key, text: &str) -> Vec<Event> {
    vec![key(key_code, true), Event::Text(text.to_owned()), key(key_code, false)]
}

fn click_at(pos: Pos2) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]
}

fn input_with(events: Vec<Event>) -> egui::RawInput {
    let mut input = raw_input(WINDOW, None);
    input.focused = true;
    input.events = events;
    input
}

/// Opens the Settings panel on the tab named by `tab` and answers the frame that
/// drew it.
fn open_tab(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    tab: super::SettingsTab,
) -> Vec<egui::epaint::ClippedShape> {
    app.toggle_settings();
    let editor = app.settings.as_mut().expect("the panel opened");
    editor.active_tab = tab;
    run_app_frame_with_input(ctx, app, input_with(Vec::new())).shapes
}

/// Focuses the field that draws `hint` and types `a`, Space, `b`, one key per
/// frame, as a person types at a calm speed. Answers the texts of the frame after
/// the last key.
fn type_a_space_b(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    shapes: &[egui::epaint::ClippedShape],
    hint: &str,
) -> Vec<String> {
    let start = super::panel_test_support::text_start(shapes, hint)
        .unwrap_or_else(|| panic!("the hint {hint:?} is drawn in the field"));
    let field = Pos2::new(start.x + 12.0, start.y + 4.0);

    run_app_frame_with_input(ctx, app, input_with(click_at(field)));
    let focused = ctx.memory(egui::Memory::focused);
    assert!(
        focused.is_some(),
        "the field that draws {hint:?} took the keyboard focus"
    );

    run_app_frame_with_input(ctx, app, input_with(typed(Key::A, "a")));
    run_app_frame_with_input(ctx, app, input_with(typed(Key::Space, " ")));
    run_app_frame_with_input(ctx, app, input_with(typed(Key::B, "b")));

    let output = run_app_frame_with_input(ctx, app, input_with(Vec::new()));
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        super::tests::collect_texts(&clipped.shape, &mut texts);
    }
    texts
}

#[test]
fn the_command_field_of_the_server_tab_keeps_a_space() {
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Server);
    let texts = type_a_space_b(&ctx, &mut app, &shapes, horizon_core::DEFAULT_LINK_COMMAND);

    let command = app
        .settings
        .as_ref()
        .expect("the panel stays open")
        .config
        .server
        .command
        .clone();
    assert_eq!(
        command.as_deref(),
        Some("a b"),
        "the space of the person reached the field"
    );
    assert!(
        super::tests::texts_hold(&texts, "a b"),
        "the field draws the text of the person: {texts:?}"
    );
}

#[test]
fn the_ssh_target_of_the_server_tab_keeps_a_space() {
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Server);
    type_a_space_b(&ctx, &mut app, &shapes, "user@host or an SSH alias");

    let target = app
        .settings
        .as_ref()
        .expect("the panel stays open")
        .config
        .server
        .ssh_target
        .clone();
    assert_eq!(
        target.as_deref(),
        Some("a b"),
        "the space of the person reached the field"
    );
}

#[test]
fn the_chromium_path_of_the_browser_tab_keeps_a_space() {
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Browser);
    type_a_space_b(&ctx, &mut app, &shapes, "/usr/bin/chromium");

    let command = app
        .settings
        .as_ref()
        .expect("the panel stays open")
        .config
        .browser
        .command
        .clone();
    assert_eq!(
        command.as_deref(),
        Some("a b"),
        "a program path of a person keeps the spaces of a Mac path"
    );
}

#[test]
fn the_name_field_of_the_secrets_tab_keeps_a_space() {
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Secrets);
    let texts = type_a_space_b(&ctx, &mut app, &shapes, "NAME");

    assert!(
        super::tests::texts_hold(&texts, "a b"),
        "the name field draws the text of the person: {texts:?}"
    );
}

#[test]
fn the_name_field_of_the_presets_tab_keeps_a_space() {
    let config = Config {
        presets: vec![PresetConfig {
            name: String::new(),
            alias: None,
            kind: PanelKind::Shell,
            command: None,
            args: Vec::new(),
            resume: PanelResume::Fresh,
            ssh_connection: None,
        }],
        ..Config::default()
    };
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(
        &config,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Presets);
    let texts = type_a_space_b(&ctx, &mut app, &shapes, "Name");

    let name = app
        .settings
        .as_ref()
        .expect("the panel stays open")
        .config
        .presets
        .first()
        .map(|preset| preset.name.clone());
    assert_eq!(
        name.as_deref(),
        Some("a b"),
        "the space of the person reached the field"
    );
    assert!(super::tests::texts_hold(&texts, "a b"), "the field draws it: {texts:?}");
}

/// A field of the panel that a person leaves keeps its text without the spaces
/// around it, and a box of nothing but spaces is an entry that is not set.
#[test]
fn a_field_that_the_person_leaves_is_trimmed() {
    let (_temp, ctx, mut app) = crate::app::test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let shapes = open_tab(&ctx, &mut app, super::SettingsTab::Server);
    let start = super::panel_test_support::text_start(&shapes, "user@host or an SSH alias")
        .expect("the hint of the SSH target is drawn");
    let field = Pos2::new(start.x + 12.0, start.y + 4.0);
    run_app_frame_with_input(&ctx, &mut app, input_with(click_at(field)));
    run_app_frame_with_input(&ctx, &mut app, input_with(typed(Key::Space, " ")));
    run_app_frame_with_input(&ctx, &mut app, input_with(typed(Key::A, "a")));
    run_app_frame_with_input(&ctx, &mut app, input_with(typed(Key::Space, " ")));

    // A press on the canvas beside the panel takes the focus away from the field.
    run_app_frame_with_input(&ctx, &mut app, input_with(click_at(Pos2::new(80.0, 600.0))));
    let target = app
        .settings
        .as_ref()
        .expect("the panel stays open")
        .config
        .server
        .ssh_target
        .clone();
    assert_eq!(
        target.as_deref(),
        Some("a"),
        "the field that was left holds the word alone"
    );

    // A box of nothing but spaces names no value at all.
    let mut app = app;
    app.settings.as_mut().expect("the panel").config.server.ssh_target = None;
    let shapes = run_app_frame_with_input(&ctx, &mut app, input_with(Vec::new())).shapes;
    let start = super::panel_test_support::text_start(&shapes, "user@host or an SSH alias")
        .expect("the hint of the empty field is drawn");
    let field = Pos2::new(start.x + 12.0, start.y + 4.0);
    run_app_frame_with_input(&ctx, &mut app, input_with(click_at(field)));
    run_app_frame_with_input(&ctx, &mut app, input_with(typed(Key::Space, " ")));
    run_app_frame_with_input(&ctx, &mut app, input_with(click_at(Pos2::new(80.0, 600.0))));
    assert_eq!(
        app.settings
            .as_ref()
            .expect("the panel stays open")
            .config
            .server
            .ssh_target,
        None,
        "a box of spaces is an entry that is not set"
    );
}
