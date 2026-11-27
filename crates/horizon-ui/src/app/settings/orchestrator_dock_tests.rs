//! The row `Orchestrator dock` of the Features section of the Settings window: the
//! setting that turns the experimental column at the right edge of the window on.

use egui::{Context, Event, Modifiers, PointerButton};
use horizon_core::Config;

use super::SettingsTab;
use super::panel_test_support::text_rects;
use super::panel_tests::settings_frame;
use super::tests::ephemeral;
use crate::app::HorizonApp;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_config_and_startup};

/// A window that is tall enough to show the row of the Features section without
/// a scroll.
const VIEWPORT: [f32; 2] = [1400.0, 2400.0];

/// The label of the row, and the line under it.
const LABEL: &str = "Orchestrator dock";
const HELP: &str = "A conversation with one agent at the right edge of the window. It does the work of the workspace and shows it as cards. Experimental.";

/// The window of a start whose settings hold `dock`, with its Settings window
/// open on the General tab.
fn settings_of(dock: bool) -> (tempfile::TempDir, Context, HorizonApp) {
    let mut config = Config::default();
    config.features.orchestrator_dock = dock;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(&config, ephemeral());
    app.root_viewport_stabilizer = None;
    app.toggle_settings();
    (temp, ctx, app)
}

/// The General tab, drawn.
fn drawn(ctx: &Context, app: &mut HorizonApp) -> Vec<egui::epaint::ClippedShape> {
    settings_frame(ctx, app, VIEWPORT, SettingsTab::General)
}

/// One press on `label`, and the frames that take it.
fn press(ctx: &Context, app: &mut HorizonApp, label: &str) {
    let shapes = drawn(ctx, app);
    let rects = text_rects(&shapes, label);
    assert_eq!(rects.len(), 1, "the row draws {label} once");
    let pos = rects[0].center();
    let mut hover = raw_input(VIEWPORT, None);
    hover.events = vec![Event::PointerMoved(pos)];
    let _ = run_app_frame_with_input(ctx, app, hover);
    for pressed in [true, false] {
        let mut input = raw_input(VIEWPORT, None);
        input.events = vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }];
        let _ = run_app_frame_with_input(ctx, app, input);
    }
}

/// The value of the row in the draft of the Settings window, and whether the
/// draft differs from the store.
fn draft(app: &HorizonApp) -> (bool, bool) {
    let editor = app.settings.as_ref().expect("the Settings window is open");
    (editor.config.features.orchestrator_dock, editor.dirty)
}

/// The row stands in the Features section with the sentence of the design, so a
/// person reads what the experiment is before they turn it on.
#[test]
fn the_features_section_shows_the_row_of_the_orchestrator_dock() {
    let (_temp, ctx, mut app) = settings_of(false);
    let shapes = drawn(&ctx, &mut app);

    assert_eq!(text_rects(&shapes, LABEL).len(), 1, "the row has its label");
    assert_eq!(text_rects(&shapes, HELP).len(), 1, "the row has its line");
    assert_eq!(draft(&app), (false, false), "a store without the setting reads it off");
}

/// A press on the row turns the setting on in the draft of the window, and a
/// second press turns it off.
#[test]
fn a_press_on_the_row_turns_the_dock_of_the_settings_on() {
    let (_temp, ctx, mut app) = settings_of(false);

    press(&ctx, &mut app, LABEL);

    assert_eq!(draft(&app), (true, true), "the press turns the setting on");
    press(&ctx, &mut app, LABEL);
    assert_eq!(draft(&app), (false, true), "the second press turns it off");
}
