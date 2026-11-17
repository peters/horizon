//! The footer of the Settings panel: its status line and its buttons.

use super::panel_test_support::{BUTTON_LABELS, for_each_painted, panel_background_rect, reserved_panel_rect};
use super::panel_tests::{app_with_stored_secrets, settings_frame};

/// The footer of the panel: its status line and its four buttons. The bar is the
/// widest shape of the last band of the panel, and every text of that band is
/// either the label of a button or the status line.
fn footer_texts(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> Vec<(String, egui::Rect)> {
    let band = footer_band(shapes, panel);
    let mut texts = Vec::new();
    for_each_painted(shapes, &mut |shape, painted, clip| {
        // A text that the clip of the tab cuts is scrolled under the bar, so the
        // person does not read it in the footer.
        if let egui::Shape::Text(drawn) = shape
            && clip.contains_rect(painted)
            && painted.center().y >= band.min.y + 4.0
            && painted.center().y <= band.max.y - 4.0
        {
            texts.push((drawn.galley.text().to_owned(), painted));
        }
    });
    texts
}

/// The band that the footer bar covers: the widest shape that ends at the bottom
/// of the panel content and is as tall as a bar of one row or of two.
fn footer_band(shapes: &[egui::epaint::ClippedShape], panel: egui::Rect) -> egui::Rect {
    let mut band = egui::Rect::NOTHING;
    for_each_painted(shapes, &mut |_shape, painted, _clip| {
        let at_the_bottom = painted.max.y <= panel.max.y + 1.0 && panel.max.y - painted.max.y <= 20.0;
        if at_the_bottom && painted.height() > 40.0 && painted.height() < 120.0 && painted.width() > band.width() {
            band = painted;
        }
    });
    assert!(!band.is_negative(), "the panel {panel:?} draws its footer bar");
    band
}

/// One state of the save that the footer shows.
#[derive(Clone, Copy)]
enum FooterState {
    None,
    LivePreview,
    Saving,
    Saved,
    Error,
}

impl FooterState {
    /// The state of the editor, as the frame that applies the save sets it.
    fn status(self, error: &str) -> super::SettingsStatus {
        match self {
            Self::None => super::SettingsStatus::None,
            Self::LivePreview => super::SettingsStatus::LivePreview,
            Self::Saving => super::SettingsStatus::Saving,
            Self::Saved => super::SettingsStatus::Saved,
            Self::Error => super::SettingsStatus::Error(error.to_owned()),
        }
    }

    /// The text of the footer: the state, or the line of an error that is longer
    /// than the place of the status line.
    fn text(self) -> Option<&'static str> {
        match self {
            Self::LivePreview => Some("Live preview"),
            Self::Saving => Some("Saving"),
            Self::Saved => Some("Saved"),
            Self::None | Self::Error => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::None => "no state",
            Self::LivePreview => "live preview",
            Self::Saving => "saving",
            Self::Saved => "saved",
            Self::Error => "an error",
        }
    }
}

/// The footer keeps its text and its buttons in the panel, whatever the tab and
/// whatever the state of the save says.
///
/// The status line takes the width that the buttons leave, so no state can put a
/// button over it: a footer that draws "Live preview" under the Close button
/// reads "Live prev" and hides the rest.
#[test]
fn the_footer_row_fits_the_panel_in_every_state() {
    let viewport = [1480.0_f32, 900.0];
    let (_temp, ctx, mut app, _names) = app_with_stored_secrets();
    let error = "The endpoint of the transcriber must be an address that starts with https, and this \
                 line of an error is longer than the place of the status line";
    let cases = [
        (FooterState::None, false),
        (FooterState::LivePreview, true),
        (FooterState::Saving, true),
        (FooterState::Saved, false),
        (FooterState::Error, true),
    ];

    for (case, dirty) in cases {
        let state = case.name();
        for tab in super::SettingsTab::ALL {
            if let Some(editor) = app.settings.as_mut() {
                editor.status = case.status(error);
                editor.dirty = dirty;
            }
            let shapes = settings_frame(&ctx, &mut app, viewport, *tab);
            let panel = panel_background_rect(&shapes, reserved_panel_rect(&app));
            let band = footer_band(&shapes, panel);
            let texts = footer_texts(&shapes, panel);
            let buttons: Vec<egui::Rect> = texts
                .iter()
                .filter(|(text, _)| BUTTON_LABELS.contains(&text.as_str()))
                .map(|(_, rect)| *rect)
                .collect();
            let expected_buttons = if dirty { 4 } else { 3 };
            assert_eq!(
                buttons.len(),
                expected_buttons,
                "the {} tab draws {expected_buttons} buttons of its footer in {state}: {texts:?}",
                tab.label()
            );

            // The buttons are in the panel, and in the row that the bar gives them.
            for button in &buttons {
                assert!(
                    button.min.x >= panel.min.x + 1.0 && button.max.x <= panel.max.x - 1.0,
                    "the {state} footer draws a button at {button:?} beside the panel {panel:?}"
                );
                assert!(
                    band.expand(1.0).contains_rect(*button),
                    "the {state} footer draws a button at {button:?} and its bar is {band:?}"
                );
            }

            // The status line is the one text of the band that is not a button, it
            // keeps the place that the buttons leave, and it shows the state.
            let lines: Vec<&(String, egui::Rect)> = texts
                .iter()
                .filter(|(text, _)| !BUTTON_LABELS.contains(&text.as_str()))
                .collect();
            let line_text = match case {
                FooterState::None => {
                    assert!(
                        lines.is_empty(),
                        "the {state} footer of the {} tab draws no line: {lines:?}",
                        tab.label()
                    );
                    continue;
                }
                FooterState::Error => error,
                other => other.text().unwrap_or_default(),
            };
            assert_eq!(
                lines.len(),
                1,
                "the {state} footer of the {} tab draws one line: {texts:?}",
                tab.label()
            );
            let (drawn, line) = lines[0];
            for button in &buttons {
                assert!(
                    !line.intersects(*button) && line.max.x + 6.0 <= button.min.x,
                    "the {state} footer draws {drawn:?} at {line:?} under the button {button:?}"
                );
            }
            assert!(
                band.expand(1.0).contains_rect(*line),
                "the {state} footer draws {drawn:?} at {line:?} and its bar is {band:?}"
            );
            assert_eq!(
                drawn,
                line_text,
                "the {} tab says the state of the save in its footer",
                tab.label()
            );
            if matches!(case, FooterState::Error) {
                // A line of an error can be longer than the place of the status
                // line. It is then cut inside that place, and the person reads the
                // whole line when the pointer rests on it.
                let whole = status_line_width(&ctx, error);
                assert!(
                    line.width() + 1.0 < whole,
                    "a line of {whole} points is cut in a place of {} points: {line:?}",
                    line.width()
                );
            }
        }
    }
}

/// The width of `text` in the font and the size of the status line of the footer.
fn status_line_width(ctx: &egui::Context, text: &str) -> f32 {
    ctx.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), egui::Color32::WHITE)
            .size()
            .x
    })
}

/// The footer gives the person the two choices of a value that another computer
/// changed while the same section is edited here: the sentence and both buttons
/// stay in the panel, the sentence is the whole line of the question, and it has a
/// row of its own above the buttons.
#[test]
fn the_footer_offers_the_two_choices_of_a_value_of_another_computer() {
    let viewport = [1480.0_f32, 900.0];
    let (_temp, ctx, mut app, _names) = app_with_stored_secrets();
    if let Some(editor) = app.settings.as_mut() {
        editor.dirty = true;
        editor.status = super::SettingsStatus::LivePreview;
        editor.conflict = Some(super::SettingsConflict {
            sections: vec!["appearance".to_owned()],
            stored: Box::new(horizon_core::Config::default()),
        });
    }
    for tab in super::SettingsTab::ALL {
        let shapes = settings_frame(&ctx, &mut app, viewport, *tab);
        let panel = panel_background_rect(&shapes, reserved_panel_rect(&app));
        // The window of 1480 points opens a panel of 444 points, which is the width
        // where the sentence and both choices no longer share one row.
        assert!(
            (panel.width() - 444.0).abs() <= 1.0,
            "the panel of this window is {panel:?}"
        );
        let band = footer_band(&shapes, panel);
        let texts = footer_texts(&shapes, panel);
        let buttons: Vec<egui::Rect> = texts
            .iter()
            .filter(|(text, _)| BUTTON_LABELS.contains(&text.as_str()))
            .map(|(_, rect)| *rect)
            .collect();
        assert_eq!(
            buttons.len(),
            2,
            "the {} tab draws the two choices in its footer: {texts:?}",
            tab.label()
        );
        for button in &buttons {
            assert!(
                band.expand(1.0).contains_rect(*button) && button.min.x >= panel.min.x + 1.0,
                "the choice at {button:?} is in the footer bar {band:?} of the panel {panel:?}"
            );
        }
        let lines: Vec<&(String, egui::Rect)> = texts
            .iter()
            .filter(|(text, _)| !BUTTON_LABELS.contains(&text.as_str()))
            .collect();
        assert_eq!(lines.len(), 1, "one line: {texts:?}");
        let (drawn, line) = lines[0];
        assert_eq!(drawn, super::CONFLICT_LINE);
        assert!(
            band.expand(1.0).contains_rect(*line),
            "the sentence at {line:?} is in the footer bar {band:?}"
        );
        // The sentence is above the two choices and not beside them, and the whole
        // of it is drawn: the row belongs to it alone.
        for button in &buttons {
            assert!(
                line.max.y <= button.min.y + 1.0,
                "the sentence at {line:?} has its own row above the choice at {button:?}"
            );
        }
        let whole = status_line_width(&ctx, super::CONFLICT_LINE);
        assert!(
            line.width() + 1.0 >= whole,
            "the sentence of {whole} points is cut to {} points at the width of this panel: {line:?}",
            line.width()
        );
    }
}

/// The rectangle of each button of the footer of `app`, with `dirty` as the state
/// of the draft and `tab` open, in a window of `viewport`.
fn button_places(
    ctx: &egui::Context,
    app: &mut crate::app::HorizonApp,
    viewport: [f32; 2],
    tab: super::SettingsTab,
    dirty: bool,
) -> Vec<(String, egui::Rect)> {
    if let Some(editor) = app.settings.as_mut() {
        editor.dirty = dirty;
        editor.status = if dirty {
            super::SettingsStatus::LivePreview
        } else {
            super::SettingsStatus::None
        };
    }
    let shapes = settings_frame(ctx, app, viewport, tab);
    let panel = panel_background_rect(&shapes, reserved_panel_rect(app));
    footer_texts(&shapes, panel)
        .into_iter()
        .filter(|(text, _)| BUTTON_LABELS.contains(&text.as_str()))
        .collect()
}

/// A change adds Revert to the footer. The places of `Save`, `Reset Defaults` and
/// `Close` stand still, because a person who goes to press Save where it stood
/// pressed Reset Defaults instead.
#[test]
fn the_places_of_save_and_reset_defaults_stand_still_when_a_change_adds_revert() {
    // The narrowest panel that a window opens, and a panel of its widest width.
    for viewport in [[1480.0_f32, 900.0], [3000.0, 900.0]] {
        let (_temp, ctx, mut app, _names) = app_with_stored_secrets();
        for tab in super::SettingsTab::ALL {
            let before = button_places(&ctx, &mut app, viewport, *tab, false);
            let after = button_places(&ctx, &mut app, viewport, *tab, true);
            assert_eq!(
                before.len(),
                3,
                "the clean footer of the {} tab: {before:?}",
                tab.label()
            );
            assert_eq!(
                after.len(),
                4,
                "the changed footer of the {} tab: {after:?}",
                tab.label()
            );
            for label in ["Save", "Reset Defaults", "Close"] {
                let place = |places: &[(String, egui::Rect)]| {
                    places
                        .iter()
                        .find(|(text, _)| text == label)
                        .map_or_else(|| panic!("the {} tab draws no {label}", tab.label()), |(_, rect)| *rect)
                };
                assert_eq!(
                    place(&before),
                    place(&after),
                    "the {} tab keeps {label} in its place in a window of {viewport:?}",
                    tab.label()
                );
            }
        }
    }
}
