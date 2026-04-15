use egui::{FontId, Margin, Stroke, Ui, Vec2};

use crate::app::yaml_highlight::highlight_yaml;
use crate::theme;

/// Render the raw YAML text editor for the configuration file.
pub(super) fn render(ui: &mut Ui, config_path: &str, buffer: &mut String, available: Vec2) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Config File").color(theme::FG_SOFT()).size(12.0));
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(config_path)
                .color(theme::FG_DIM())
                .size(11.0)
                .monospace(),
        );
    });
    ui.add_space(8.0);

    let font_id = FontId::monospace(13.0);
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
        let job = highlight_yaml(text.as_str(), &ui.style().text_styles[&egui::TextStyle::Monospace]);
        ui.fonts_mut(|f| f.layout_job(job))
    };

    egui::Frame::default()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(8)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(available.y - 48.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(buffer)
                            .font(font_id)
                            .desired_width(available.x)
                            .desired_rows(40)
                            .frame(false)
                            .layouter(&mut layouter),
                    );
                });
        });
}
