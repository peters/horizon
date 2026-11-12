//! A person opens a viewer of a frame source program from the Remote tab.
//!
//! The button and the row are the path of the person; the checked path and its
//! sentences are those of the `device_panel` operation of an agent, so these tests
//! press the button of the real tab and read the panels and the sentences of the
//! whole app.
use super::SettingsTab;
use super::panel_test_support::for_each_painted;
use super::panel_tests::settings_frame;
use super::tests::{ephemeral, texts_hold};
use crate::app::test_support::test_app_with_startup;

/// The position of one drawn word of the settings panel of the last frame.
fn word_center(shapes: &[egui::epaint::ClippedShape], label: &str) -> egui::Pos2 {
    let mut found = None;
    for_each_painted(shapes, &mut |shape, painted, _clip| {
        if let egui::Shape::Text(drawn) = shape
            && drawn.galley.text() == label
        {
            found = Some(painted.center());
        }
    });
    found.unwrap_or_else(|| panic!("missing the word {label}"))
}

/// One frame of the whole app with the events of a person.
fn app_frame(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    viewport: [f32; 2],
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    let mut input = crate::app::test_support::raw_input(viewport, None);
    input.events = events;
    crate::app::test_support::run_app_frame_with_input(ctx, app, input)
}

/// Press one point with the primary button, as a person does.
fn press_at(ctx: &egui::Context, app: &mut crate::app::HorizonApp, viewport: [f32; 2], at: egui::Pos2) {
    app_frame(ctx, app, viewport, vec![egui::Event::PointerMoved(at)]);
    for pressed in [true, false] {
        app_frame(
            ctx,
            app,
            viewport,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
}

#[test]
fn the_remote_tab_opens_a_frame_source_program_of_this_computer() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    let folder = tempfile::tempdir().expect("a folder of a test");
    let program = folder.path().join("a-frame-source-program");
    std::fs::write(&program, b"#!/bin/sh\nexit 0\n").expect("a program of a test");
    let mut mode = std::fs::metadata(&program).expect("a program of a test").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&program, mode).expect("a mode of a test");
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.frame_sources.sources.push(horizon_core::FrameSource {
            name: "bench".to_owned(),
            command: format!("{} {{target}}", program.display()),
        });
    }
    let viewport = [1480.0, 900.0];
    let shapes = settings_frame(&ctx, &mut app, viewport, SettingsTab::RemoteBrowsers);
    let open = word_center(&shapes, "Open");
    press_at(&ctx, &mut app, viewport, open);
    let panel = app
        .board
        .panels
        .iter()
        .find(|panel| {
            panel
                .device()
                .and_then(|device| device.target.frame_source_ref())
                .is_some_and(|reference| reference.name == "bench")
        })
        .expect("the press opened one viewer of the program");
    assert_eq!(panel.kind, horizon_core::PanelKind::Device);
    assert_eq!(
        panel.layout.size,
        horizon_core::FRAME_SOURCE_PANEL_SIZE,
        "a viewer opens at the size that draws its first picture wide enough to read"
    );
}

#[test]
fn a_program_that_this_computer_does_not_hold_says_so_under_its_row() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    app.toggle_settings();
    if let Some(editor) = app.settings.as_mut() {
        editor.config.frame_sources.sources.push(horizon_core::FrameSource {
            name: "bench".to_owned(),
            command: "/no/folder/of/this/computer/a-frame-source-program {target}".to_owned(),
        });
    }
    let viewport = [1480.0, 900.0];
    let shapes = settings_frame(&ctx, &mut app, viewport, SettingsTab::RemoteBrowsers);
    let open = word_center(&shapes, "Open");
    press_at(&ctx, &mut app, viewport, open);
    let shapes = settings_frame(&ctx, &mut app, viewport, SettingsTab::RemoteBrowsers);
    let mut texts = Vec::new();
    for_each_painted(&shapes, &mut |shape, _, _| {
        if let egui::Shape::Text(drawn) = shape {
            texts.push(drawn.galley.text().to_owned());
        }
    });
    assert!(
        texts_hold(
            &texts,
            "/no/folder/of/this/computer/a-frame-source-program is not installed on this computer"
        ),
        "the row says why it opened nothing: {texts:?}"
    );
}
