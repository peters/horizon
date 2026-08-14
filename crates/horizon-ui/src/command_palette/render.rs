use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::app::util::usize_to_f32;
use crate::badge::paint_badge_background;
use crate::text::{painter_text_galley, single_line_label_job};
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
    paint_badge_background(
        painter,
        card_rect,
        CornerRadius::same(20),
        theme::PANEL_BG(),
        Some((
            Stroke::new(1.5_f32, theme::alpha(theme::ACCENT(), 80)),
            StrokeKind::Outside,
        )),
    );
    painter.rect_stroke(
        card_rect.expand(2.0),
        CornerRadius::same(22),
        Stroke::new(2.0_f32, theme::alpha(theme::ACCENT(), 25)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_empty_results(ui: &mut egui::Ui, message: &str) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(message).color(theme::FG_DIM()).size(12.0));
    });
}

pub(super) fn render_section_header(ui: &mut egui::Ui, width: f32, title: &str) {
    let rect = ui.allocate_space(Vec2::new(width, SECTION_HEADER_HEIGHT)).1;
    ui.painter_at(rect).text(
        Pos2::new(rect.min.x + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(10.5),
        theme::FG_DIM(),
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
    let painter = ui.painter_at(row_rect);

    if is_selected {
        painter.rect_filled(
            row_rect,
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("pal_hover", index)), Sense::hover())
            .hovered();
        if hover {
            painter.rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT(), 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("pal_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;

    let label_x = if let Some(color) = item.accent {
        painter.circle_filled(Pos2::new(row_rect.min.x + 14.0, text_y), 4.5, color);
        row_rect.min.x + 30.0
    } else {
        row_rect.min.x + 10.0
    };
    let shortcut_galley = item.shortcut.as_ref().map(|shortcut| {
        painter_text_galley(
            &painter,
            shortcut,
            &egui::FontId::monospace(10.0),
            theme::FG_DIM(),
            (row_rect.width() - 28.0).max(0.0),
        )
    });
    let reserved_shortcut_width = shortcut_galley.as_ref().map_or(0.0, |galley| galley.size().x + 28.0);
    let content_right = row_rect.max.x - 12.0 - reserved_shortcut_width;
    let label_color = if is_selected { theme::FG() } else { theme::FG_SOFT() };
    let label_galley = painter_text_galley(
        &painter,
        &item.label,
        &egui::FontId::proportional(13.0),
        label_color,
        (content_right - label_x).max(0.0),
    );
    let label_size = label_galley.size();
    let label_rect = Rect::from_min_max(
        Pos2::new(label_x, row_rect.min.y),
        Pos2::new(content_right.max(label_x), row_rect.max.y),
    );
    painter.with_clip_rect(label_rect).galley(
        Pos2::new(label_x, text_y - label_size.y * 0.5),
        label_galley,
        Color32::TRANSPARENT,
    );

    if !item.detail.is_empty() {
        let detail_x = label_x + label_size.x + 12.0;
        if detail_x < content_right {
            let detail_galley = painter.layout_job(single_line_label_job(
                item.detail.trim(),
                &egui::FontId::proportional(11.0),
                theme::FG_DIM(),
                content_right - detail_x,
            ));
            let detail_rect = Rect::from_min_max(
                Pos2::new(detail_x, row_rect.min.y),
                Pos2::new(content_right, row_rect.max.y),
            );
            painter.with_clip_rect(detail_rect).galley(
                Pos2::new(detail_x, text_y - detail_galley.size().y * 0.5),
                detail_galley,
                Color32::TRANSPARENT,
            );
        }
    }

    if let Some(galley) = shortcut_galley {
        paint_shortcut_badge(&painter, row_rect, text_y, galley);
    }

    clicked
}

fn paint_shortcut_badge(
    painter: &egui::Painter,
    row_rect: Rect,
    text_y: f32,
    shortcut_galley: std::sync::Arc<egui::Galley>,
) {
    let shortcut_size = shortcut_galley.size();
    let badge_width = shortcut_size.x + 12.0;
    let badge_rect = Rect::from_min_size(
        Pos2::new(row_rect.max.x - badge_width - 8.0, text_y - 10.0),
        Vec2::new(badge_width, 20.0),
    );
    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(5),
        theme::alpha(theme::BG_ELEVATED(), 200),
        Some((
            Stroke::new(0.5_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
            StrokeKind::Inside,
        )),
    );
    painter.galley(
        badge_rect.center() - shortcut_size * 0.5,
        shortcut_galley,
        theme::FG_DIM(),
    );
}
