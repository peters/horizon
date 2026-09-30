//! Choice buttons and the GPU types a new cloud would request, shared by the creation
//! fields.
use crate::theme;
use egui::{Button, Color32, FontId, TextFormat, Ui, Vec2, text::LayoutJob};
use horizon_core::{cloud_panel::Placement, cloud_runtime::prices::Preferences};

/// A choice button with an optional second line, such as a price or stock, drawn in
/// `tint` or else in the accent color when selected.
pub(super) fn option(ui: &mut Ui, label: &str, selected: bool, detail: Option<&str>, tint: Option<Color32>) -> bool {
    ui.add(button(label, selected, detail, tint)).clicked()
}

pub(super) fn button(label: &str, selected: bool, detail: Option<&str>, tint: Option<Color32>) -> Button<'static> {
    let mut job = LayoutJob::default();
    let format = |size: f32, color: Color32| TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..TextFormat::default()
    };
    job.append(label, 0.0, format(13.0, theme::FG()));
    if let Some(detail) = detail {
        let color = tint.unwrap_or(if selected { theme::ACCENT() } else { theme::FG_DIM() });
        job.append(&format!("\n{detail}"), 0.0, format(11.0, color));
    }
    Button::new(job)
        .selected(selected)
        .min_size(Vec2::new(0.0, if detail.is_some() { 42.0 } else { 30.0 }))
        .corner_radius(8)
}

/// The GPU types a new cloud would request: those chosen for it, or else the machine's
/// preferences.
pub(super) fn requested_gpus<'a>(preferences: &'a Preferences, placement: &'a Placement) -> &'a [String] {
    if placement.gpu_types.is_empty() {
        &preferences.gpu_types
    } else {
        &placement.gpu_types
    }
}
