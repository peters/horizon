use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::theme;

pub(super) fn paint_workspace_frame(ui: &mut egui::Ui, rect: Rect, color: Color32, is_active: bool) {
    let painter = ui.painter_at(rect);
    let corner_radius = CornerRadius::same(20);
    let border_alpha = if is_active { 110 } else { 55 };
    let fill_alpha = if is_active { 24 } else { 14 };
    let frame_fill = theme::alpha(theme::blend(theme::PANEL_BG, color, 0.12), fill_alpha);

    painter.rect_filled(rect, corner_radius, frame_fill);
    painter.rect_stroke(
        rect,
        corner_radius,
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_workspace_label_bg(
    ui: &mut egui::Ui,
    rect: Rect,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    let painter = ui.painter();
    let tint = if dragging {
        0.22
    } else if hovered {
        0.18
    } else if is_active {
        0.14
    } else {
        0.08
    };
    let fill = theme::blend(theme::PANEL_BG_ALT, color, tint);
    let border_alpha = if is_active || hovered { 160 } else { 90 };

    painter.rect_filled(rect, CornerRadius::same(10), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0, theme::alpha(color, border_alpha)),
        StrokeKind::Outside,
    );
}

pub(super) fn paint_workspace_label(
    ui: &mut egui::Ui,
    rect: Rect,
    name: &str,
    color: Color32,
    is_active: bool,
    hovered: bool,
    dragging: bool,
) {
    paint_workspace_label_bg(ui, rect, color, is_active, hovered, dragging);

    let painter = ui.painter();
    let grip_center = Pos2::new(rect.max.x - 14.0, rect.center().y);

    painter.circle_filled(
        Pos2::new(rect.min.x + 14.0, rect.center().y),
        4.0,
        theme::alpha(color, if is_active { 220 } else { 150 }),
    );

    painter.text(
        Pos2::new(rect.min.x + 26.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.5),
        if is_active { theme::FG } else { theme::FG_SOFT },
    );

    paint_workspace_grip(painter, grip_center, dragging || hovered);
}

fn paint_workspace_grip(painter: &egui::Painter, center: Pos2, highlighted: bool) {
    let color = if highlighted {
        theme::alpha(theme::FG_SOFT, 180)
    } else {
        theme::alpha(theme::FG_DIM, 140)
    };
    let x_offsets = [-3.0, 3.0];
    let y_offsets = [-4.0, 0.0, 4.0];

    for x_offset in x_offsets {
        for y_offset in y_offsets {
            painter.circle_filled(Pos2::new(center.x + x_offset, center.y + y_offset), 1.2, color);
        }
    }
}

pub(super) fn paint_empty_workspace_hint(ui: &mut egui::Ui, rect: Rect, label_rect: Rect, color: Color32) {
    let painter = ui.painter();
    let copy_pos = Pos2::new(rect.min.x + 18.0, label_rect.max.y + 22.0);

    painter.text(
        copy_pos,
        egui::Align2::LEFT_TOP,
        "Drag this workspace anywhere on the board.",
        egui::FontId::proportional(12.0),
        theme::alpha(theme::FG_SOFT, 210),
    );
    painter.text(
        copy_pos + Vec2::new(0.0, 20.0),
        egui::Align2::LEFT_TOP,
        "New terminals will land inside this frame.",
        egui::FontId::proportional(10.5),
        theme::alpha(theme::blend(theme::FG_DIM, color, 0.18), 196),
    );
}
