use egui::{Color32, FontId, Pos2, Rect, Ui, Vec2};

use super::painter_text_galley;

pub(crate) fn paint_section_header(ui: &mut Ui, width: f32, height: f32, title: &str, font: &FontId, color: Color32) {
    let rect = ui.allocate_space(Vec2::new(width, height)).1;
    let painter = ui.painter_at(rect);
    let text_x = rect.min.x + 4.0;
    let max_x = (rect.max.x - 4.0).max(text_x);
    let galley = painter_text_galley(&painter, title, font, color, (max_x - text_x).max(0.0));
    let text_rect = Rect::from_min_max(Pos2::new(text_x, rect.min.y), Pos2::new(max_x, rect.max.y));
    painter.with_clip_rect(text_rect).galley(
        Pos2::new(text_x, rect.center().y - galley.size().y * 0.5),
        galley,
        Color32::TRANSPARENT,
    );
}
