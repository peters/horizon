use egui::{Area, CursorIcon, Id, LayerId, Pos2, Rect, Response, Sense, Ui, Vec2};
use horizon_core::PanelId;

use super::super::PANEL_TITLEBAR_HEIGHT;
use super::CanvasClippedArea;
use crate::theme;

const HANDLE_SCREEN_SIZE: f32 = 32.0;
const GRIP_DOT_RADIUS: f32 = 1.45;
/// Positions in the 32-point handle, measured from its top-left.
const GRIP_DOT_COLUMNS: [f32; 2] = [18.0, 24.0];
const GRIP_DOT_ROWS: [f32; 3] = [17.0, 21.5, 26.0];

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

fn grip_scale(rect: Rect) -> f32 {
    rect.width() / HANDLE_SCREEN_SIZE
}

fn grip_dot_radius(rect: Rect) -> f32 {
    GRIP_DOT_RADIUS * grip_scale(rect)
}

fn grip_dot_centers(rect: Rect) -> [Pos2; 6] {
    let scale = grip_scale(rect);
    let mut centers = [Pos2::ZERO; 6];
    let mut index = 0;
    for row in GRIP_DOT_ROWS {
        for column in GRIP_DOT_COLUMNS {
            centers[index] = rect.min + Vec2::new(column, row) * scale;
            index += 1;
        }
    }
    centers
}

fn paint_control(ui: &Ui, rect: Rect, response: &Response, interactive: bool) {
    let active = interactive && (response.hovered() || response.dragged());
    let color = if active { theme::ACCENT() } else { theme::FG_DIM() };
    let radius = grip_dot_radius(rect);
    let painter = ui.painter();
    for center in grip_dot_centers(rect) {
        painter.circle_filled(center, radius, color);
    }
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
    }
}

#[cfg(test)]
mod tests;
