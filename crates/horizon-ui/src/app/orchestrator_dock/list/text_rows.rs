//! The rows of the words of a conversation: the bubble of the person, and the text of
//! the agent with the bar at its left.

use egui::{Align, Label, Mesh, Rect, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

use crate::theme;
use crate::typeface::Weight;

use super::super::markdown;
use super::super::text;
use super::{
    AGENT_BAR_END_ACCENT, AGENT_BAR_RADIUS, AGENT_BAR_WIDTH, AGENT_GAP, AGENT_LINE_HEIGHT, AGENT_SHARE, BODY_SIZE,
    BUBBLE_PADDING_X, BUBBLE_PADDING_Y, BUBBLE_RADIUS, PERSON_LINE_HEIGHT, PERSON_SHARE, VIA_GAP,
};

/// One message of the person: a bubble at the right of the list, with the caption
/// of the front desk above it when the front desk passed the message.
pub(in super::super) fn person_row(ui: &mut Ui, text: &str, caption: Option<&str>, caption_size: f32, width: f32) {
    let text_width = (width * PERSON_SHARE - 2.0 * BUBBLE_PADDING_X).max(1.0);
    let galley = text::paragraph(
        ui,
        text,
        BODY_SIZE,
        Weight::Regular,
        theme::FG(),
        text_width,
        PERSON_LINE_HEIGHT,
    );
    let bubble = vec2(
        galley.size().x + 2.0 * BUBBLE_PADDING_X,
        galley.size().y + 2.0 * BUBBLE_PADDING_Y,
    );
    let caption = caption.map(|caption| text::line(ui, caption, caption_size, Weight::Regular, theme::FG_DIM(), width));
    let caption_height = caption.as_ref().map_or(0.0, |caption| caption.size().y + VIA_GAP);
    let (row, _) = ui.allocate_exact_size(vec2(width, bubble.y + caption_height), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.max.x - bubble.x, row.min.y + caption_height), bubble);
    if let Some(caption) = caption {
        ui.painter()
            .galley(pos2(rect.max.x - caption.size().x, row.min.y), caption, theme::FG_DIM());
    }
    let painter = ui.painter();
    painter.rect(
        rect,
        BUBBLE_RADIUS,
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.min.x + BUBBLE_PADDING_X, rect.min.y + BUBBLE_PADDING_Y),
        galley,
        theme::FG(),
    );
}

/// One text of the agent: the accent bar at the left of the list, and the text
/// beside it, selectable, as plain text. The bar runs the full height of the text
/// and steps from the accent at its top to the mauve of the workspace accents at
/// its bottom.
pub(in super::super) fn agent_row(ui: &mut Ui, text: &str, caption: Option<&str>, caption_size: f32, width: f32) {
    let indent = AGENT_BAR_WIDTH + AGENT_GAP;
    let text_width = (width * AGENT_SHARE - indent).max(1.0);
    // The marks of Markdown are drawn, not shown: an agent writes them every day.
    let job = markdown::job(text, BODY_SIZE, AGENT_LINE_HEIGHT, theme::FG(), text_width);
    let text_height = ui.painter().layout_job(job.clone()).size().y;
    // A caption stands above the text of the agent, at the left edge of the list.
    let caption = caption.map(|caption| text::line(ui, caption, caption_size, Weight::Regular, theme::FG_DIM(), width));
    let caption_height = caption.as_ref().map_or(0.0, |caption| caption.size().y + VIA_GAP);
    let height = text_height + caption_height;
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if let Some(caption) = caption {
        ui.painter().galley(row.min, caption, theme::FG_DIM());
    }
    let top = row.min.y + caption_height;
    agent_bar(
        ui.painter(),
        Rect::from_min_size(pos2(row.min.x, top), vec2(AGENT_BAR_WIDTH, text_height)),
    );
    let text_rect = Rect::from_min_size(pos2(row.min.x + indent, top), vec2(text_width, text_height));
    let mut text_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
    );
    text_ui.add(Label::new(job).selectable(true).wrap());
}

/// The bar of one agent text: a mesh of two colours that runs the full height of
/// the text, with a round cap at each end for its radius.
fn agent_bar(painter: &egui::Painter, rect: Rect) {
    let top = theme::ACCENT();
    let bottom = theme::workspace_accent(AGENT_BAR_END_ACCENT);
    let cap = (f32::from(AGENT_BAR_RADIUS)).min(AGENT_BAR_WIDTH / 2.0);
    let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + cap), pos2(rect.max.x, rect.max.y - cap));
    let mut mesh = Mesh::default();
    mesh.colored_vertex(body.left_top(), top);
    mesh.colored_vertex(body.right_top(), top);
    mesh.colored_vertex(body.left_bottom(), bottom);
    mesh.colored_vertex(body.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(Shape::mesh(mesh));
    painter.circle_filled(pos2(rect.center().x, body.min.y), cap, top);
    painter.circle_filled(pos2(rect.center().x, body.max.y), cap, bottom);
}
