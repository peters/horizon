//! Conversions for layout and animation, where a rounded value is all that is needed.

use egui::Ui;

/// A count as a float. Counts here are tiny (tiles, bars, steps), so saturating is harmless.
pub(super) fn count(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Seconds since the app started, wrapped so the float keeps its precision for animation.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn seconds(ui: &Ui) -> f32 {
    (ui.input(|input| input.time) % 3600.0) as f32
}

/// A non-negative float rounded down to an index.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn index(value: f32) -> usize {
    value.max(0.0) as usize
}
