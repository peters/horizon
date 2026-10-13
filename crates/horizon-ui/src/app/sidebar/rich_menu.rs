//! Menu rows that say what they do: an optional glyph tile, a title, a one-line
//! description, an optional note under it and an optional shortcut. Used by New workspace and the toolbar Menu.
use std::f32::consts::PI;

use egui::{
    Align, Color32, CornerRadius, CursorIcon, Layout, Painter, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2,
};

use crate::theme;

/// A small mark drawn in the row's tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Glyph {
    Cloud,
    Screen,
}

const TILE: f32 = 30.0;
/// The height a note adds to a row.
const NOTE_HEIGHT: f32 = 15.0;

/// One row. `note` is a dimmer line under `detail`, such as a machine and its price.
/// Returns its response; disabled rows still answer hover for their reason.
pub(in crate::app) fn row(
    ui: &mut Ui,
    glyph: Option<Glyph>,
    title: &str,
    detail: &str,
    note: Option<&str>,
    shortcut: Option<&str>,
    enabled: bool,
) -> egui::Response {
    let text_height = if note.is_some() { 34.0 + NOTE_HEIGHT } else { 34.0 };
    let height = text_height + if glyph.is_some() { 16.0 } else { 10.0 };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), sense);
    let response = if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, title));
    let hot = enabled && (response.hovered() || response.has_focus());
    if hot {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.20),
        );
    }
    let mut left = rect.min.x + 10.0;
    if let Some(glyph) = glyph {
        let tile_rect = Rect::from_min_size(Pos2::new(left, rect.center().y - TILE / 2.0), Vec2::splat(TILE));
        let tint = if hot { 0.42 } else { 0.26 };
        ui.painter().rect_filled(
            tile_rect,
            CornerRadius::same(9),
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), tint),
        );
        let ink = if enabled { theme::FG() } else { theme::FG_DIM() };
        paint(ui.painter(), glyph, tile_rect.center(), ink);
        left = tile_rect.max.x + 12.0;
    }
    let inner = Rect::from_min_max(Pos2::new(left, rect.min.y), Pos2::new(rect.max.x - 10.0, rect.max.y));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            if !enabled {
                ui.disable();
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add_space((height - text_height) / 2.0);
                ui.add(egui::Label::new(RichText::new(title).size(13.5).strong().color(theme::FG())).selectable(false));
                ui.add(
                    egui::Label::new(RichText::new(detail).size(11.5).color(theme::FG_SOFT()))
                        .truncate()
                        .selectable(false),
                );
                if let Some(note) = note {
                    ui.add(
                        egui::Label::new(RichText::new(note).size(11.5).color(theme::FG_DIM()))
                            .truncate()
                            .selectable(false),
                    );
                }
            });
            if let Some(shortcut) = shortcut {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(theme::BG_ELEVATED())
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(shortcut).size(10.0).monospace().color(theme::FG_SOFT()),
                                )
                                .selectable(false),
                            );
                        });
                });
            }
        },
    );
    response
}

/// A thin divider between groups of rows.
pub(in crate::app) fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment(
        [
            Pos2::new(rect.min.x + 8.0, rect.center().y),
            Pos2::new(rect.max.x - 8.0, rect.center().y),
        ],
        Stroke::new(0.5, theme::alpha(theme::BORDER_SUBTLE(), 180)),
    );
}

/// A short caption above a group of rows.
pub(in crate::app) fn caption(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.add(
            egui::Label::new(
                RichText::new(text.to_uppercase())
                    .size(10.5)
                    .strong()
                    .color(theme::FG_DIM()),
            )
            .selectable(false),
        );
    });
    ui.add_space(2.0);
}

pub(in crate::app) fn paint(painter: &Painter, glyph: Glyph, center: Pos2, ink: Color32) {
    match glyph {
        Glyph::Cloud => cloud(painter, center, ink),
        Glyph::Screen => screen(painter, center, ink),
    }
}

fn arc(points: &mut Vec<Pos2>, center: Pos2, radius: f32, from: f32, to: f32) {
    for step in 0..=10_u8 {
        let t = from + (to - from) * f32::from(step) / 10.0;
        points.push(Pos2::new(center.x + radius * t.cos(), center.y + radius * t.sin()));
    }
}

fn cloud(painter: &Painter, center: Pos2, ink: Color32) {
    let base = center.y + 4.0;
    let mut points = Vec::new();
    arc(
        &mut points,
        Pos2::new(center.x - 3.5, base - 3.0),
        3.0,
        PI * 0.5,
        PI * 1.5,
    );
    arc(
        &mut points,
        Pos2::new(center.x + 0.5, base - 4.5),
        4.5,
        PI * 1.05,
        PI * 1.95,
    );
    arc(
        &mut points,
        Pos2::new(center.x + 4.5, base - 2.5),
        2.5,
        PI * 1.5,
        PI * 2.5,
    );
    points.push(Pos2::new(center.x - 3.5, base));
    painter.add(Shape::closed_line(points, Stroke::new(1.4, ink)));
}

fn screen(painter: &Painter, center: Pos2, ink: Color32) {
    let display = Rect::from_center_size(center - Vec2::new(0.0, 1.5), Vec2::new(13.0, 9.0));
    painter.rect_stroke(display, 1, Stroke::new(1.4, ink), StrokeKind::Middle);
    painter.line_segment(
        [
            Pos2::new(center.x - 3.5, center.y + 5.5),
            Pos2::new(center.x + 3.5, center.y + 5.5),
        ],
        Stroke::new(1.4, ink),
    );
}
