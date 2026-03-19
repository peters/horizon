use egui::{Align, CornerRadius, Layout, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::app::util::usize_to_f32;
use crate::theme;

use super::{
    BADGE_FONT, DETAIL_FONT, INPUT_HEIGHT, LABEL_FONT, MAX_VISIBLE_ROWS, ROW_HEIGHT, SEARCH_WIDTH,
    SECTION_HEADER_HEIGHT,
};

pub(super) struct SearchLayout {
    pub screen: Rect,
    pub card: Rect,
    pub inner: Rect,
    pub results_height: f32,
}

pub(super) fn search_layout(screen: Rect) -> SearchLayout {
    let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT + 4.0 * SECTION_HEADER_HEIGHT;
    let card_height = INPUT_HEIGHT + 16.0 + results_height + 80.0;
    let card_min = Pos2::new(
        (screen.width() - SEARCH_WIDTH) * 0.5,
        (screen.height() - card_height) * 0.25,
    );
    let card = Rect::from_min_size(card_min, Vec2::new(SEARCH_WIDTH, card_height));

    SearchLayout {
        screen,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        results_height,
    }
}

pub(super) fn paint_card(ui: &egui::Ui, card_rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(card_rect, CornerRadius::same(20), theme::PANEL_BG);
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(20),
        Stroke::new(1.5, theme::alpha(theme::ACCENT, 80)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        card_rect.expand(2.0),
        CornerRadius::same(22),
        Stroke::new(2.0, theme::alpha(theme::ACCENT, 25)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_empty_results(ui: &mut egui::Ui, message: &str) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(message).color(theme::FG_DIM).size(12.0));
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
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("search_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("search_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;
    let label_x = row_rect.min.x + 10.0;

    // Panel title
    ui.painter_at(row_rect).text(
        Pos2::new(label_x, text_y),
        egui::Align2::LEFT_CENTER,
        data.panel_title,
        LABEL_FONT,
        if is_selected { theme::ACCENT } else { theme::FG_SOFT },
    );

    // Line preview
    let title_width = estimate_text_width(data.panel_title, 13.0);
    let detail_x = label_x + title_width + 12.0;
    let max_detail_x = row_rect.max.x - badge_width(data.match_count_label.as_ref()) - 8.0;

    if detail_x < max_detail_x {
        let available = max_detail_x - detail_x;
        let truncated = truncate_to_width(data.line_text.trim(), available, 11.0);
        ui.painter_at(row_rect).text(
            Pos2::new(detail_x, text_y),
            egui::Align2::LEFT_CENTER,
            &truncated,
            DETAIL_FONT,
            theme::FG_DIM,
        );
    }

    // Match count badge
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

    let size = Vec2::new(estimate_text_width(label, 11.0) + 16.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    ui.painter().rect_filled(rect, CornerRadius::same(6), bg);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE, 180)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
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
        format!("{total_matches} {matches_word} across {panel_count} {panels_word}")
    };

    let mut child = ui.new_child(UiBuilder::new().layout(Layout::left_to_right(Align::Center)));
    child.label(egui::RichText::new(text).color(theme::FG_DIM).size(11.0));
}

fn paint_count_badge(ui: &egui::Ui, row_rect: Rect, text_y: f32, label: &str) {
    let badge_width = estimate_text_width(label, 10.0) + 12.0;
    let badge_rect = Rect::from_min_size(
        Pos2::new(row_rect.max.x - badge_width - 8.0, text_y - 10.0),
        Vec2::new(badge_width, 20.0),
    );
    ui.painter_at(row_rect)
        .rect_filled(badge_rect, CornerRadius::same(5), theme::alpha(theme::BG_ELEVATED, 200));
    ui.painter_at(row_rect).rect_stroke(
        badge_rect,
        CornerRadius::same(5),
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
    label.map_or(0.0, |s| estimate_text_width(s, 10.0) + 28.0)
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
