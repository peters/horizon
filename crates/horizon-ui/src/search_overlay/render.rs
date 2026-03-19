use egui::{Align, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::app::util::usize_to_f32;
use crate::theme;

use super::{BADGE_FONT, DETAIL_FONT, LABEL_FONT, ROW_HEIGHT, SECTION_HEADER_HEIGHT};

pub(super) fn paint_dropdown_frame(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(14), theme::PANEL_BG);
    painter.rect_stroke(
        rect,
        CornerRadius::same(14),
        Stroke::new(1.0, theme::alpha(theme::ACCENT, 60)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect.expand(1.5),
        CornerRadius::same(15),
        Stroke::new(1.5, theme::alpha(theme::ACCENT, 18)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_empty_results(ui: &mut egui::Ui, message: &str) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(message).color(theme::FG_DIM).size(11.0));
    });
}

pub(super) fn render_section_header(ui: &mut egui::Ui, width: f32, title: &str) {
    let rect = ui.allocate_space(Vec2::new(width, SECTION_HEADER_HEIGHT)).1;
    ui.painter_at(rect).text(
        Pos2::new(rect.min.x + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        BADGE_FONT,
        theme::FG_DIM,
    );
}

pub(super) struct MatchRowData<'a> {
    pub panel_title: &'a str,
    pub line_text: &'a str,
    pub match_count_label: Option<String>,
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

    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(6),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("search_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(6),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("search_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;
    let label_x = row_rect.min.x + 8.0;

    if !data.panel_title.is_empty() {
        ui.painter_at(row_rect).text(
            Pos2::new(label_x, text_y),
            egui::Align2::LEFT_CENTER,
            data.panel_title,
            LABEL_FONT,
            if is_selected { theme::ACCENT } else { theme::FG_SOFT },
        );
    }

    let title_width = if data.panel_title.is_empty() {
        0.0
    } else {
        estimate_text_width(data.panel_title, 12.0) + 10.0
    };
    let detail_x = label_x + title_width;
    let max_detail_x = row_rect.max.x - badge_width(data.match_count_label.as_ref()) - 6.0;

    if detail_x < max_detail_x {
        let available = max_detail_x - detail_x;
        let truncated = truncate_to_width(data.line_text.trim(), available, 10.5);
        ui.painter_at(row_rect).text(
            Pos2::new(detail_x, text_y),
            egui::Align2::LEFT_CENTER,
            &truncated,
            DETAIL_FONT,
            theme::FG_DIM,
        );
    }

    if let Some(count_label) = &data.match_count_label {
        paint_count_badge(ui, row_rect, text_y, count_label);
    }

    clicked
}

pub(super) fn render_toggle_button(ui: &mut egui::Ui, label: &str, active: bool, _id_salt: &str) -> bool {
    let (fg, bg) = if active {
        (theme::FG, theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.35))
    } else {
        (theme::FG_DIM, theme::BG_ELEVATED)
    };

    let size = Vec2::new(estimate_text_width(label, 11.0) + 14.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE, 180)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(10.0),
        fg,
    );

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
    child.label(egui::RichText::new(text).color(theme::FG_DIM).size(10.0));
}

fn paint_count_badge(ui: &egui::Ui, row_rect: Rect, text_y: f32, label: &str) {
    let badge_w = estimate_text_width(label, 9.5) + 10.0;
    let badge_rect = Rect::from_min_size(
        Pos2::new(row_rect.max.x - badge_w - 6.0, text_y - 9.0),
        Vec2::new(badge_w, 18.0),
    );
    ui.painter_at(row_rect)
        .rect_filled(badge_rect, CornerRadius::same(4), theme::alpha(theme::BG_ELEVATED, 200));
    ui.painter_at(row_rect).rect_stroke(
        badge_rect,
        CornerRadius::same(4),
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE, 180)),
        StrokeKind::Inside,
    );
    ui.painter_at(row_rect).text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        BADGE_FONT,
        theme::FG_DIM,
    );
}

fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    let char_width = font_size * 0.58;
    usize_to_f32(text.len()) * char_width
}

fn badge_width(label: Option<&String>) -> f32 {
    label.map_or(0.0, |s| estimate_text_width(s, 9.5) + 24.0)
}

fn truncate_to_width(text: &str, max_width: f32, font_size: f32) -> String {
    let char_width = font_size * 0.58;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let max_chars = (max_width / char_width) as usize;

    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut result: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    result.push('\u{2026}');
    result
}
