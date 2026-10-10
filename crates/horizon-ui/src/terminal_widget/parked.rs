//! The last screen of a parked cloud member, drawn as passive text in the terminal's
//! font: no cursor, no selection and no scrollback. A click on it focuses the panel,
//! which brings the cloud into use so that it attaches again.
use egui::{Align2, Sense, Ui, vec2};
use horizon_core::ParkedScreen;

use super::grid_metrics;
use crate::theme;

/// Draws `screen` in the body of its panel. Returns whether the body was clicked.
pub(crate) fn show(ui: &mut Ui, screen: &ParkedScreen) -> bool {
    let metrics = grid_metrics(ui.ctx());
    let response = ui.allocate_response(ui.available_size(), Sense::click());
    let rect = response.rect;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL_BG());
    let rows = screen.size().0.max(1);
    for (row, line) in screen.lines().iter().take(usize::from(rows)).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let offset = metrics.line_height * f32::from(u16::try_from(row).unwrap_or(u16::MAX));
        if rect.top() + offset > rect.bottom() {
            break;
        }
        painter.text(
            rect.left_top() + vec2(0.0, offset),
            Align2::LEFT_TOP,
            line,
            metrics.font_id.clone(),
            theme::FG_DIM(),
        );
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures as _;

    #[test]
    fn the_last_screen_shows_as_text_without_its_blank_rows() {
        let screen = ParkedScreen::new(
            vec![
                "$ cargo test".into(),
                String::new(),
                "running 3 tests".into(),
                "below the rows".into(),
            ],
            3,
            80,
        );
        let mut clicked = true;
        let output = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| clicked = show(ui, &screen))
            .discard_textures();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["$ cargo test", "running 3 tests"]);
        assert!(!clicked);
    }
}
