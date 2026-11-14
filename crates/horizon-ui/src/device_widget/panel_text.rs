//! The text of the device panel, made for the size it has on the screen.
//!
//! The viewer is a panel of the canvas, so its labels, its rows and its buttons
//! are drawn under the layer transform of the canvas zoom. A stock widget paints
//! a picture of zoom 1 there and the transform stretches that picture, so every
//! letter of it is resampled at any zoom but 100 percent. Every text of this
//! folder therefore goes through [`crate::zoom_text::widget_text`], which lays it
//! out at the size of the screen and keeps the size, the lines and the ellipsis
//! of zoom 1.

use egui::{Color32, FontId, TextStyle, Ui, WidgetText};

use crate::theme;
use crate::zoom_text;

/// The size of a caption, a row label and a control of the panel.
pub(super) const SMALL: f32 = 12.0;
/// The size of a section title and of a value of a row.
pub(super) const VALUE: f32 = 13.0;
/// The size of the name of the desktop at the top of the panel.
pub(super) const NAME: f32 = 20.0;

/// One line of `text`, elided at `width` when it does not fit.
pub(super) fn line(ui: &Ui, text: &str, size: f32, color: Color32, width: f32) -> WidgetText {
    zoom_text::widget_text(ui.painter(), text, FontId::proportional(size), color, Some(width))
}

/// One line of `text` that takes the room it asks for.
pub(super) fn word(ui: &Ui, text: &str, size: f32, color: Color32) -> WidgetText {
    zoom_text::widget_text(ui.painter(), text, FontId::proportional(size), color, None)
}

/// `text` wrapped inside `width`.
pub(super) fn wrapped(ui: &Ui, text: &str, size: f32, color: Color32, width: f32) -> WidgetText {
    zoom_text::wrapped_widget_text(ui.painter(), text, FontId::proportional(size), color, width)
}

/// `text` wrapped inside `width` in the monospace face of the panel.
pub(super) fn mono_wrapped(ui: &Ui, text: &str, size: f32, color: Color32, width: f32) -> WidgetText {
    zoom_text::wrapped_widget_text(ui.painter(), text, FontId::monospace(size), color, width)
}

/// The body face of the interface, which the two long labels of the header use.
pub(super) fn body_font(ui: &Ui) -> FontId {
    TextStyle::Body.resolve(ui.style())
}

/// The colour that a stock label paints without one: the foreground of the theme.
pub(super) fn fg() -> Color32 {
    theme::FG()
}

/// The colour that `ui.weak` paints.
pub(super) fn weak(ui: &Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

/// The heading of a section that a person folds, in the face of a stock button.
pub(super) fn heading(ui: &Ui, text: &str) -> WidgetText {
    let font = TextStyle::Button.resolve(ui.style());
    zoom_text::widget_text(ui.painter(), text, font, fg(), None)
}
