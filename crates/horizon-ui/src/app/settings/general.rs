use egui::Ui;
use horizon_core::Config;

use crate::theme;

/// Render the General settings tab: window dimensions, feature toggles,
/// and overlay sizes.  Returns `true` when any value was modified.
pub(super) fn render(ui: &mut Ui, config: &mut Config) -> bool {
    let mut changed = false;

    // -- Window ----------------------------------------------------------
    super::section_heading(ui, "Window");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Default window size and position on launch.");
        ui.add_space(8.0);

        egui::Grid::new("settings_window_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Width").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.width)
                            .range(400.0..=8000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Height").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.height)
                            .range(300.0..=5000.0)
                            .speed(2.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("X").color(theme::FG_SOFT).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.x, "px");

                ui.label(egui::RichText::new("Y").color(theme::FG_SOFT).size(12.0));
                changed |= optional_f32_drag(ui, &mut config.window.y, "px");
                ui.end_row();
            });
    });

    // -- Features --------------------------------------------------------
    super::section_heading(ui, "Features");
    super::section_card(ui, |ui| {
        changed |= ui
            .checkbox(
                &mut config.features.attention_feed,
                egui::RichText::new("Attention Feed").color(theme::FG).size(12.0),
            )
            .changed();
        super::dim_label(ui, "Show a notification feed for agent activity.");
    });

    // -- Overlays --------------------------------------------------------
    super::section_heading(ui, "Overlays");
    super::section_card(ui, |ui| {
        super::dim_label(ui, "Dimensions of overlay widgets on the canvas.");
        ui.add_space(8.0);

        egui::Grid::new("settings_overlays_grid")
            .num_columns(4)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Feed Width").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.attention_feed_width)
                            .range(120.0..=800.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Feed Height").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.attention_feed_height)
                            .range(100.0..=1200.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();

                ui.label(egui::RichText::new("Map Width").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_width)
                            .range(80.0..=600.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();

                ui.label(egui::RichText::new("Map Height").color(theme::FG_SOFT).size(12.0));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.overlays.minimap_height)
                            .range(60.0..=500.0)
                            .speed(1.0)
                            .suffix(" px"),
                    )
                    .changed();
                ui.end_row();
            });
    });

    changed
}

/// Render a `DragValue` for an `Option<f32>`.  An unchecked checkbox
/// clears the value to `None`.
fn optional_f32_drag(ui: &mut Ui, value: &mut Option<f32>, suffix: &str) -> bool {
    let mut changed = false;
    let mut enabled = value.is_some();

    if ui.checkbox(&mut enabled, "").changed() {
        *value = if enabled { Some(0.0) } else { None };
        changed = true;
    }

    if let Some(v) = value.as_mut() {
        changed |= ui
            .add(
                egui::DragValue::new(v)
                    .range(-10000.0..=10000.0)
                    .speed(1.0)
                    .suffix(format!(" {suffix}")),
            )
            .changed();
    }

    changed
}
