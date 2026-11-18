//! The row `When the window is closed` of the Settings window, in a window that
//! holds a session lease and in one that holds none. A second start finds a
//! window through that lease, so a window without one cannot keep running
//! hidden: the row shows that value and takes no press on it.

use egui::{Context, Event, Modifiers, PointerButton};
use horizon_core::{Config, WindowCloseSetting};

use super::SettingsTab;
use super::general::CANNOT_HIDE;
use super::panel_test_support::text_rects;
use super::panel_tests::settings_frame;
use super::tests::ephemeral;
use crate::app::HorizonApp;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_config_and_startup};

/// A window that is tall enough to show the row without a scroll.
const VIEWPORT: [f32; 2] = [1400.0, 2400.0];

/// The window of a start with `--ephemeral`, whose settings hold `setting`, with
/// its Settings window open. `lease` gives the window the session of a normal
/// start instead.
fn settings_of(setting: WindowCloseSetting, lease: bool) -> (tempfile::TempDir, Context, HorizonApp) {
    let mut config = Config::default();
    config.features.when_window_closes = setting;
    let (temp, ctx, mut app) = test_app_with_config_and_startup(&config, ephemeral());
    app.root_viewport_stabilizer = None;
    if lease {
        app.active_session = Some(crate::app::ActiveSession {
            session_id: "settings-of-a-window".to_string(),
            lease: None,
            lease_answers: Vec::new(),
            lease_written: true,
            last_lease_refresh: None,
            persistent: true,
        });
    }
    assert_eq!(app.holds_session_lease(), lease);
    app.toggle_settings();
    (temp, ctx, app)
}

/// The General tab, drawn.
fn drawn(ctx: &Context, app: &mut HorizonApp) -> Vec<egui::epaint::ClippedShape> {
    settings_frame(ctx, app, VIEWPORT, SettingsTab::General)
}

/// One press on the value `label` of the row, and the frames that take it.
fn press_value(ctx: &Context, app: &mut HorizonApp, label: &str) {
    let shapes = drawn(ctx, app);
    let rects = text_rects(&shapes, label);
    assert_eq!(rects.len(), 1, "the row draws the value {label} once");
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
fn draft(app: &HorizonApp) -> (WindowCloseSetting, bool) {
    let editor = app.settings.as_ref().expect("the Settings window is open");
    (editor.config.features.when_window_closes, editor.dirty)
}

/// A window without a lease shows `Keep Horizon running` and the line that says
/// why it is not a choice there. A press on it changes nothing, and the other
/// values stay choices.
#[test]
fn a_window_without_a_lease_cannot_choose_to_keep_running() {
    let (_temp, ctx, mut app) = settings_of(WindowCloseSetting::Ask, false);

    let shapes = drawn(&ctx, &mut app);
    assert_eq!(text_rects(&shapes, CANNOT_HIDE).len(), 1, "the row says why");

    press_value(&ctx, &mut app, WindowCloseSetting::KeepRunning.label());
    assert_eq!(
        draft(&app),
        (WindowCloseSetting::Ask, false),
        "a press on the value that hides the window changes nothing"
    );

    press_value(&ctx, &mut app, WindowCloseSetting::Quit.label());
    assert_eq!(
        draft(&app),
        (WindowCloseSetting::Quit, true),
        "the other values stay choices"
    );
}

/// A window with a lease chooses the value as before, and its row has no such
/// line.
#[test]
fn a_window_with_a_lease_chooses_to_keep_running() {
    let (_temp, ctx, mut app) = settings_of(WindowCloseSetting::Ask, true);

    let shapes = drawn(&ctx, &mut app);
    assert!(
        text_rects(&shapes, CANNOT_HIDE).is_empty(),
        "no line of a window that cannot hide"
    );

    press_value(&ctx, &mut app, WindowCloseSetting::KeepRunning.label());
    assert_eq!(draft(&app), (WindowCloseSetting::KeepRunning, true));
}

/// The Settings window of a window without a lease changes no stored value by
/// its drawing: a store that says `keep_running` keeps that value.
#[test]
fn the_settings_of_a_window_without_a_lease_keep_the_stored_value() {
    let (_temp, ctx, mut app) = settings_of(WindowCloseSetting::KeepRunning, false);

    let shapes = drawn(&ctx, &mut app);

    assert_eq!(text_rects(&shapes, CANNOT_HIDE).len(), 1, "the row says why");
    assert_eq!(
        draft(&app),
        (WindowCloseSetting::KeepRunning, false),
        "the draft holds the stored value, and nothing waits for a save"
    );
    assert_eq!(
        app.template_config.features.when_window_closes,
        WindowCloseSetting::KeepRunning,
        "the settings of this run hold the stored value"
    );
}
