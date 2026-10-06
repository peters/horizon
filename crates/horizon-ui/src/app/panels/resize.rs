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
            paint_control(ui, rect, &response, interactive, zoom);
            response.on_hover_text("Drag to resize panel")
        })
        .inner
}

/// Screen-point offsets inward from the panel corner, and the screen-point radius.
///
/// The radius follows the canvas zoom. A hit target smaller than 32 screen points
/// shifts the mark into the corner. It scales the mark down only when the corner
/// cannot hold the full dots.
fn grip_screen_geometry(screen_span: f32) -> ([Vec2; 6], f32) {
    let from_right = GRIP_DOT_COLUMNS.map(|column| HANDLE_SCREEN_SIZE - column);
    let from_bottom = GRIP_DOT_ROWS.map(|row| HANDLE_SCREEN_SIZE - row);
    let column_span = from_right[0] - from_right[1];
    let row_span = from_bottom[0] - from_bottom[2];
    let cluster = column_span.max(row_span) + 2.0 * GRIP_DOT_RADIUS;
    let scale = if screen_span < cluster {
        (screen_span / cluster).max(0.0)
    } else {
        1.0
    };
    let radius = GRIP_DOT_RADIUS * scale;
    let shift_x = (from_right[0] * scale + radius - screen_span).max(0.0);
    let shift_y = (from_bottom[0] * scale + radius - screen_span).max(0.0);
    let mut offsets = [Vec2::ZERO; 6];
    let mut index = 0;
    for row in from_bottom {
        for column in from_right {
            offsets[index] = Vec2::new(column * scale - shift_x, row * scale - shift_y);
            index += 1;
        }
    }
    (offsets, radius)
}

fn grip_dots(rect: Rect, zoom: f32) -> ([Pos2; 6], f32) {
    let (offsets, screen_radius) = grip_screen_geometry(rect.width() * zoom);
    let radius = screen_radius / zoom;
    let limit = (rect.width().min(rect.height()) * 0.5).max(0.0);
    let radius = radius.min(limit);
    let inward = Vec2::splat(radius);
    let min = rect.min + inward;
    let max = rect.max - inward;
    let mut centers = [Pos2::ZERO; 6];
    for (index, offset) in offsets.into_iter().enumerate() {
        let center = rect.max - offset / zoom;
        centers[index] = Pos2::new(center.x.clamp(min.x, max.x), center.y.clamp(min.y, max.y));
    }
    (centers, radius)
}

fn paint_control(ui: &Ui, rect: Rect, response: &Response, interactive: bool, zoom: f32) {
    let active = interactive && (response.hovered() || response.dragged());
    let color = if active { theme::ACCENT() } else { theme::FG_DIM() };
    let (centers, radius) = grip_dots(rect, zoom);
    let painter = ui.painter();
    for center in centers {
        painter.circle_filled(center, radius, color);
    }
    if active {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
    }
}

#[cfg(test)]
mod tests;
