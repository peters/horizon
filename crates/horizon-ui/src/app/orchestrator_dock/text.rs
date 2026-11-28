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

/// One line of `text` in the monospace font of the interface, cut with an
/// ellipsis at `max_width`. A tool name, a tool summary and a notice are system
/// lines, so they keep the fixed advance of `JetBrains Mono`.
pub(super) fn mono_line(ui: &Ui, text: &str, size: f32, color: Color32, max_width: f32) -> Arc<Galley> {
    let job = single_line_label_job(text, &FontId::monospace(size), color, max_width);
    layout(ui, job, Weight::Regular)
}

/// The metrics of one row or paragraph of text: its size, its weight, its colour
/// and its line height as a factor of the size.
#[derive(Clone, Copy)]
pub(super) struct Metrics {
    pub(super) size: f32,
    pub(super) weight: Weight,
    pub(super) color: Color32,
    pub(super) line_height: f32,
}

/// `text` wrapped at `max_width` in at most `rows` rows.
pub(super) fn rows(ui: &Ui, text: &str, metrics: Metrics, max_width: f32, rows: usize) -> Arc<Galley> {
    let Metrics {
        size,
        weight,
        color,
        line_height,
    } = metrics;
    let mut job = wrapped_rows_job(text, &FontId::proportional(size), color, max_width, rows);
    for section in &mut job.sections {
        section.format.line_height = Some(size * line_height);
    }
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
