use egui::{Area, CursorIcon, Id, LayerId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_core::PanelId;

use super::super::PANEL_TITLEBAR_HEIGHT;
use super::CanvasClippedArea;
use crate::theme;

const HANDLE_SCREEN_SIZE: f32 = 32.0;

pub(super) fn drag_delta(response: &Response) -> Vec2 {
    if response.drag_started() {
        // A press can share a frame with hover motion that preceded the press.
        response.total_drag_delta().unwrap_or_default()
    } else {
        response.drag_delta()
    }
}

fn handle_rect(panel: Rect, zoom: f32) -> Rect {
    let span = (HANDLE_SCREEN_SIZE / zoom)
        .min(panel.width())
        .min((panel.height() - PANEL_TITLEBAR_HEIGHT).max(0.0));
    Rect::from_min_size(panel.max - Vec2::splat(span), Vec2::splat(span))
}

pub(super) fn show_resize_control(
    ui: &mut Ui,
    panel: Rect,
    zoom: f32,
    panel_id: PanelId,
    interactive: bool,
) -> Response {
    let rect = handle_rect(panel, zoom);
    let parent = ui.layer_id();
    let layer = LayerId::new(parent.order, Id::new(("panel_resize_layer", panel_id.0)));
    let ctx = ui.ctx();
    // Body input also routes raw events by topmost layer, independently of Response.
    ctx.set_sublayer(parent, layer);
    ctx.set_transform_layer(layer, ctx.layer_transform_to_global(parent).unwrap_or_default());
    Area::new(layer.id)
        .order(layer.order)
        .fade_in(false)
        .fixed_pos(rect.min)
        .constrain(false)
        .clip_to_canvas(ui.clip_rect())
        .interactable(interactive)
        .sense(Sense::hover())
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
            let response = ui.interact(
                rect,
                ui.make_persistent_id(("panel_resize", panel_id.0)),
                if interactive { Sense::drag() } else { Sense::hover() },
            );
            paint_control(ui, rect, &response, interactive);
            response.on_hover_text("Drag to resize panel")
        })
        .inner
}

fn paint_control(ui: &Ui, rect: Rect, response: &Response, interactive: bool) {
    let active = interactive && (response.hovered() || response.dragged());
    let color = if active { theme::ACCENT() } else { theme::FG_DIM() };
    let scale = rect.width() / HANDLE_SCREEN_SIZE;
    let painter = ui.painter();
    painter.rect(
        rect,
        4.0 * scale,
        theme::blend(theme::PANEL_BG_ALT(), color, if active { 0.18 } else { 0.06 }),
        Stroke::new(scale, if active { color } else { theme::BORDER_SUBTLE() }),
        StrokeKind::Inside,
    );
    let corner = rect.max - Vec2::splat(7.0 * scale);
    for length in [6.0, 12.0, 18.0] {
        painter.line_segment(
            [
                Pos2::new(corner.x - length * scale, corner.y),
                Pos2::new(corner.x, corner.y - length * scale),
            ],
            Stroke::new(1.5 * scale, color),
        );
    }
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
    }
}

#[cfg(test)]
mod tests;
