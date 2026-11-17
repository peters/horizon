//! The Settings panel in a small window: at the smallest window that the build
//! opens, the panel and every tab of it stand inside the window, no line crosses
//! the panel, and Escape closes it.

use egui::{Event, Key, Modifiers, Rect};

use super::panel_test_support::{for_each_painted, panel_background_rect, reserved_panel_rect};
use super::panel_tests::{app_with_stored_secrets, assert_tab_fits_the_panel, seed_server_tab, settings_frame};
use super::tests::ephemeral;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

#[test]
fn every_tab_of_the_panel_stands_inside_a_small_window() {
    for viewport in [[800.0_f32, 600.0], [1024.0, 640.0]] {
        let (temp, ctx, mut app, _names) = app_with_stored_secrets();
        seed_server_tab(&mut app, temp.path());
        let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::from(viewport));
        for tab in super::SettingsTab::ALL {
            let label = tab.label();
            // The panel draws its edges and nothing else from its top to its
            // bottom: a line of that height between the edges crosses the tabs.
            let shapes = settings_frame(&ctx, &mut app, viewport, *tab);
            let reserved = reserved_panel_rect(&app);
            let panel = panel_background_rect(&shapes, reserved);
            for_each_painted(&shapes, &mut |shape, _, _| {
                if let egui::Shape::LineSegment { points, .. } = shape {
                    let upright = (points[0].x - points[1].x).abs() < 0.5
                        && (points[0].y - points[1].y).abs() > panel.height() / 2.0;
                    assert!(
                        !upright || points[0].x <= panel.min.x + 2.0 || points[0].x >= panel.max.x - 2.0,
                        "the {label} tab draws the line {points:?} across the panel {panel:?}"
                    );
                }
            });
            assert!(
                screen.expand(1.0).contains_rect(panel) && screen.contains_rect(reserved),
                "the {label} tab draws the panel {panel:?} on the room {reserved:?} inside the window {screen:?}"
            );
            let _ = assert_tab_fits_the_panel(&ctx, &mut app, viewport, *tab);
        }
    }
}

/// A press of `key` and its release, each in one frame of a window of 800 by
/// 600 points.
fn press(ctx: &egui::Context, app: &mut crate::app::HorizonApp, key: Key) {
    for pressed in [true, false] {
        let mut input = raw_input([800.0, 600.0], None);
        input.events = vec![Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        let _ = run_app_frame_with_input(ctx, app, input);
    }
}

#[test]
fn escape_closes_the_panel_as_close_does() {
    let (_temp, ctx, mut app) = test_app_with_startup(ephemeral());
    app.initial_pan_done = true;
    app.root_viewport_stabilizer = None;
    app.toggle_settings();
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input([800.0, 600.0], None));
    press(&ctx, &mut app, Key::Escape);
    assert!(app.settings.is_none(), "Escape closes the panel that holds no change");

    // A changed field: Escape drops the draft and shows the saved settings again,
    // which is what Close does.
    app.toggle_settings();
    let saved = app.template_config.overlays.minimap_width;
    let mut draft = app.template_config.clone();
    draft.overlays.minimap_width = saved + 40.0;
    if let Some(editor) = app.settings.as_mut() {
        editor.config = draft.clone();
        editor.dirty = true;
    }
    app.apply_live_preview(&draft);
    press(&ctx, &mut app, Key::Escape);
    assert!(app.settings.is_none(), "Escape closes the panel that holds a change");
    assert!(
        (app.template_config.overlays.minimap_width - saved).abs() < f32::EPSILON,
        "the draft of the closed panel is dropped, as Close drops it"
    );

    // An open list of the panel and a list above the panel take the key first,
    // and the panel stays.
    app.toggle_settings();
    egui::Popup::open_id(&ctx, egui::Id::unique("a list of the panel"));
    press(&ctx, &mut app, Key::Escape);
    assert!(app.settings.is_some(), "Escape belongs to the open list of the panel");
    app.open_command_palette();
    press(&ctx, &mut app, Key::Escape);
    assert!(
        app.command_palette.is_none() && app.settings.is_some(),
        "Escape closes the palette above the panel, and the panel stays"
    );
}
