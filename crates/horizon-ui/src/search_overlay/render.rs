use egui::{Align, Color32, CornerRadius, Layout, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::badge::paint_badge_background;
use crate::text::{painter_text_galley, single_line_label_job};
use crate::theme;

use super::{BADGE_FONT, DETAIL_FONT, LABEL_FONT, ROW_HEIGHT, SECTION_HEADER_HEIGHT};

pub(super) fn paint_toolbar_search_input(ui: &egui::Ui, rect: Rect, focused: bool, hovered: bool, has_query: bool) {
    let painter = ui.painter();
    let glow_alpha = if focused {
        34
    } else if hovered {
        20
    } else {
        10
    };
    let shell_fill = theme::blend(
        theme::PANEL_BG(),
        theme::ACCENT(),
        if focused {
            0.14
        } else if hovered {
            0.08
        } else {
            0.04
        },
    );
    let core_fill = theme::blend(
        theme::BG_ELEVATED(),
        theme::ACCENT(),
        if focused {
            0.16
        } else if hovered {
            0.09
        } else {
            0.05
        },
    );
    let border = theme::alpha(
        theme::blend(
            theme::BORDER_SUBTLE(),
            theme::ACCENT(),
            if focused {
                0.78
            } else if hovered {
                0.5
            } else {
                0.32
            },
        ),
        if focused { 240 } else { 210 },
    );
    let icon_fill = if focused || has_query {
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.4)
    } else {
        theme::alpha(theme::PANEL_BG_ALT(), 230)
    };
    let icon_stroke = if focused || hovered {
        theme::alpha(theme::blend(theme::BORDER_STRONG(), theme::ACCENT(), 0.58), 230)
    } else {
        theme::alpha(theme::BORDER_SUBTLE(), 220)
    };
    let icon_color = if focused || has_query {
        theme::FG()
    } else {
        theme::FG_SOFT()
    };

    painter.rect_stroke(
        rect.expand(3.0),
        CornerRadius::same(13),
        Stroke::new(3.0_f32, theme::alpha(theme::ACCENT(), glow_alpha)),
        StrokeKind::Outside,
    );
    painter.rect_filled(rect, CornerRadius::same(10), shell_fill);

    let core_rect = rect.shrink(1.0);
    painter.rect_filled(core_rect, CornerRadius::same(9), core_fill);
    painter.rect_stroke(
        core_rect,
        CornerRadius::same(9),
        Stroke::new(1.0_f32, border),
        StrokeKind::Inside,
    );

    painter.line_segment(
        [
            Pos2::new(core_rect.min.x + 16.0, core_rect.min.y + 1.5),
            Pos2::new(core_rect.max.x - 16.0, core_rect.min.y + 1.5),
        ],
        Stroke::new(1.0_f32, theme::alpha(theme::FG(), if focused { 28 } else { 16 })),
    );

    let badge_rect = Rect::from_center_size(
        Pos2::new(core_rect.min.x + 24.0, core_rect.center().y),
        Vec2::new(22.0, 22.0),
    );
    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(7),
        icon_fill,
        Some((Stroke::new(1.0_f32, icon_stroke), StrokeKind::Inside)),
    );
    paint_search_icon(painter, badge_rect.center(), icon_color);
}

pub(super) fn paint_dropdown_frame(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), theme::PANEL_BG());
    painter.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), 60)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect.expand(1.5),
        CornerRadius::same(15),
        Stroke::new(1.5_f32, theme::alpha(theme::ACCENT(), 18)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_empty_results(ui: &mut egui::Ui, message: &str) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(message).color(theme::FG_DIM()).size(11.0));
    });
}

pub(super) fn render_section_header(ui: &mut egui::Ui, width: f32, title: &str) {
    let rect = ui.allocate_space(Vec2::new(width, SECTION_HEADER_HEIGHT)).1;
    ui.painter_at(rect).text(
        Pos2::new(rect.min.x + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        BADGE_FONT,
        theme::FG_DIM(),
    );
}

pub(super) struct MatchRowData<'a> {
    pub panel_title: &'a str,
    pub line_text: &'a str,
    pub match_count_label: Option<&'a str>,
}

pub(super) fn render_match_row(
    ui: &mut egui::Ui,
    width: f32,
    index: usize,
    data: &MatchRowData<'_>,
    is_selected: bool,
) -> bool {
    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;
    let painter = ui.painter_at(row_rect);

    if is_selected {
        painter.rect_filled(
            row_rect,
            CornerRadius::same(6),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("search_hover", index)), Sense::hover())
            .hovered();
        if hover {
            painter.rect_filled(
                row_rect,
                CornerRadius::same(6),
                theme::alpha(theme::PANEL_BG_ALT(), 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("search_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;
    let label_x = row_rect.min.x + 8.0;
    let title_color = if is_selected { theme::ACCENT() } else { theme::FG_SOFT() };
    let title_galley = (!data.panel_title.is_empty())
        .then(|| painter_text_galley(&painter, data.panel_title, LABEL_FONT, title_color));
    let title_width = title_galley.as_ref().map_or(0.0, |galley| galley.size().x + 10.0);
    if let Some(galley) = title_galley {
        let position = Pos2::new(label_x, text_y - galley.size().y * 0.5);
        painter.galley(position, galley, title_color);
    }

    let count_galley = data
        .match_count_label
        .map(|label| painter_text_galley(&painter, label, BADGE_FONT, theme::FG_DIM()));
    let reserved_badge_width = count_galley.as_ref().map_or(0.0, |galley| galley.size().x + 24.0);
    let detail_x = label_x + title_width;
    let max_detail_x = row_rect.max.x - reserved_badge_width - 6.0;

    if detail_x < max_detail_x {
        let available = max_detail_x - detail_x;
        let detail_galley = painter.layout_job(search_detail_layout_job(data.line_text, available));
        let detail_rect = Rect::from_min_max(
            Pos2::new(detail_x, row_rect.min.y),
            Pos2::new(max_detail_x, row_rect.max.y),
        );
        painter.with_clip_rect(detail_rect).galley(
            Pos2::new(detail_x, text_y - detail_galley.size().y * 0.5),
            detail_galley,
            Color32::TRANSPARENT,
        );
    }

    if let Some(galley) = count_galley {
        paint_count_badge(&painter, row_rect, text_y, galley);
    }

    clicked
}

pub(super) fn render_toggle_button(ui: &mut egui::Ui, label: &str, active: bool, _id_salt: &str) -> bool {
    let (fg, bg) = if active {
        (theme::FG(), theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.35))
    } else {
        (theme::FG_DIM(), theme::BG_ELEVATED())
    };

    let label_galley = painter_text_galley(ui.painter(), label, egui::FontId::proportional(10.0), fg);
    let label_size = label_galley.size();
    let size = Vec2::new(label_size.x + 14.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(0.5_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
        StrokeKind::Inside,
    );
    ui.painter().galley(rect.center() - label_size * 0.5, label_galley, fg);

    response.clicked()
}

pub(super) fn render_status_line(ui: &mut egui::Ui, total_matches: usize, panel_count: usize) {
    let text = if total_matches == 0 {
        "No matches".to_string()
    } else {
        let panels_word = if panel_count == 1 { "panel" } else { "panels" };
        let matches_word = if total_matches == 1 { "match" } else { "matches" };
        format!("{total_matches} {matches_word} in {panel_count} {panels_word}")
    };

    let mut child = ui.new_child(UiBuilder::new().layout(Layout::left_to_right(Align::Center)));
    child.label(egui::RichText::new(text).color(theme::FG_DIM()).size(10.0));
}

fn paint_count_badge(painter: &Painter, row_rect: Rect, text_y: f32, label_galley: std::sync::Arc<egui::Galley>) {
    let label_size = label_galley.size();
    let badge_w = label_size.x + 10.0;
    let badge_rect = Rect::from_min_size(
        Pos2::new(row_rect.max.x - badge_w - 6.0, text_y - 9.0),
        Vec2::new(badge_w, 18.0),
    );
    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(4),
        theme::alpha(theme::BG_ELEVATED(), 200),
        Some((
            Stroke::new(0.5_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
            StrokeKind::Inside,
        )),
    );
    painter.galley(badge_rect.center() - label_size * 0.5, label_galley, theme::FG_DIM());
}

fn paint_search_icon(painter: &Painter, center: Pos2, color: Color32) {
    let loop_center = Pos2::new(center.x - 1.5, center.y - 1.5);
    painter.circle_stroke(loop_center, 4.5, Stroke::new(1.35_f32, color));
    painter.line_segment(
        [
            Pos2::new(loop_center.x + 3.5, loop_center.y + 3.5),
            Pos2::new(loop_center.x + 7.0, loop_center.y + 7.0),
        ],
        Stroke::new(1.35_f32, color),
    );
}

fn search_detail_layout_job(text: &str, max_width: f32) -> egui::text::LayoutJob {
    single_line_label_job(text.trim(), &DETAIL_FONT, theme::FG_DIM(), max_width)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::search_detail_layout_job;

    #[test]
    fn detail_layout_uses_real_glyph_width_and_single_line_elision() {
        let ctx = egui::Context::default();
        let width = Cell::new(0.0_f32);
        let elided = Cell::new(false);

        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let galley = ui
                    .painter()
                    .layout_job(search_detail_layout_job("wide 二二二二二二二二二二", 40.0));
                width.set(galley.size().x);
                elided.set(galley.elided);
            });
        });

        assert!(width.get() <= 40.0, "detail exceeded pixel budget: {}", width.get());
        assert!(elided.get());
        assert_eq!(search_detail_layout_job(" first\r\nsecond ", 40.0).text, "first second");
    }
}
