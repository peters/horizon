//! The text of the dock: one line that ends with an ellipsis, and a paragraph
//! that wraps at a line height.
//!
//! The helpers of a card live in `block_widget`, which no other module can see,
//! so the dock lays its text out with the shared jobs of `crate::text` and the
//! one weight table of the interface. Layout still goes through egui's galley
//! cache, so a text is shaped once while it and its font stay the same.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Color32, FontId, Galley, Ui};

use crate::text::{single_line_label_job, wrapped_rows_job};
use crate::typeface::Weight;

/// One line of `text` at `weight`, cut with an ellipsis at `max_width`.
pub(super) fn line(ui: &Ui, text: &str, size: f32, weight: Weight, color: Color32, max_width: f32) -> Arc<Galley> {
    let job = single_line_label_job(text, &FontId::proportional(size), color, max_width);
    layout(ui, job, weight)
}

/// `text` wrapped at `max_width` in `size * line_height_factor` rows.
pub(super) fn paragraph(
    ui: &Ui,
    text: &str,
    size: f32,
    weight: Weight,
    color: Color32,
    max_width: f32,
    line_height_factor: f32,
) -> Arc<Galley> {
    let mut job = wrapped_rows_job(text, &FontId::proportional(size), color, max_width, usize::MAX);
    for section in &mut job.sections {
        section.format.line_height = Some(size * line_height_factor);
    }
    layout(ui, job, weight)
}

/// The galley of `job` at the `wght` coordinate of `weight`.
fn layout(ui: &Ui, mut job: LayoutJob, weight: Weight) -> Arc<Galley> {
    for section in &mut job.sections {
        section.format.coords = weight.coords();
    }
    ui.painter().layout_job(job)
}
