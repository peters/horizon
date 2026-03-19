use egui::{CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::app::util::usize_to_f32;
use crate::theme;

use super::{INPUT_HEIGHT, MAX_VISIBLE_ROWS, PALETTE_WIDTH, ROW_HEIGHT, ResultItem, SECTION_HEADER_HEIGHT};

pub(super) struct PaletteLayout {
    pub screen: Rect,
    pub card: Rect,
    pub inner: Rect,
    pub results_height: f32,
}

pub(super) fn palette_layout(screen: Rect) -> PaletteLayout {
    let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT + 4.0 * SECTION_HEADER_HEIGHT;
    let card_height = INPUT_HEIGHT + 16.0 + results_height + 60.0;
    let card_min = Pos2::new(
        (screen.width() - PALETTE_WIDTH) * 0.5,
        (screen.height() - card_height) * 0.25,
    );
    let card = Rect::from_min_size(card_min, Vec2::new(PALETTE_WIDTH, card_height));

    PaletteLayout {
        screen,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        results_height,
    }
}

pub(crate) fn paint_card(ui: &egui::Ui, card_rect: Rect) {
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
        egui::FontId::proportional(10.5),
        theme::FG_DIM,
    );
}

pub(super) fn render_result_row(
    ui: &mut egui::Ui,
    width: f32,
    index: usize,
    item: &ResultItem,
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
            .interact(row_rect, ui.make_persistent_id(("pal_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("pal_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;

    let label_x = if let Some(color) = item.accent {
        ui.painter_at(row_rect)
            .circle_filled(Pos2::new(row_rect.min.x + 14.0, text_y), 4.5, color);
        row_rect.min.x + 30.0
    } else {
        row_rect.min.x + 10.0
    };

    ui.painter_at(row_rect).text(
        Pos2::new(label_x, text_y),
        egui::Align2::LEFT_CENTER,
        &item.label,
        egui::FontId::proportional(13.0),
        if is_selected { theme::FG } else { theme::FG_SOFT },
    );

    if !item.detail.is_empty() {
        let detail_x = label_x + estimate_text_width(&item.label, 13.0) + 12.0;
        let max_detail_x = row_rect.max.x - 12.0 - shortcut_width(item.shortcut.as_ref());
        if detail_x < max_detail_x {
            ui.painter_at(row_rect).text(
                Pos2::new(detail_x, text_y),
                egui::Align2::LEFT_CENTER,
                &item.detail,
                egui::FontId::proportional(11.0),
                theme::FG_DIM,
            );
        }
    }

    if let Some(shortcut) = &item.shortcut {
        paint_shortcut_badge(ui, row_rect, text_y, shortcut);
    }

    clicked
}

fn paint_shortcut_badge(ui: &egui::Ui, row_rect: Rect, text_y: f32, shortcut: &str) {
    let badge_width = estimate_text_width(shortcut, 10.0) + 12.0;
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
        shortcut,
        egui::FontId::monospace(10.0),
        theme::FG_DIM,
    );
}

fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    let char_width = font_size * 0.58;
    usize_to_f32(text.len()) * char_width
}

fn shortcut_width(shortcut: Option<&String>) -> f32 {
    shortcut.map_or(0.0, |s| estimate_text_width(s, 10.0) + 28.0)
}
