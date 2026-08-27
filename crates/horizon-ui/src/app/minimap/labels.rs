use std::{cmp::Ordering, collections::HashMap, f32::consts::FRAC_PI_2, sync::Arc};

use egui::{
    Align2, Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, epaint::TextShape,
};
use horizon_core::WorkspaceId;

use crate::{text::single_line_label_job, theme};

use super::{
    HorizonApp, MinimapModel, MinimapScope, WS_TITLE_HEIGHT, scope_includes_workspace, workspace_minimap_screen_rect,
};

pub(super) struct MinimapWorkspaceLabel<'a> {
    name: &'a str,
    color: Color32,
    is_active: bool,
    workspace_rect: Rect,
    title_strip_rect: Option<Rect>,
}

pub(super) fn paint_minimap_workspace_labels(
    app: &HorizonApp,
    painter: &Painter,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) {
    let mut labels = collect_minimap_workspace_labels(app, origin, model, workspace_bounds, scope);
    labels.sort_by(minimap_workspace_label_order);

    let mut occupied = Vec::with_capacity(labels.len());
    for label in labels {
        paint_minimap_workspace_label(painter, &label, &mut occupied);
    }
}

fn collect_minimap_workspace_labels<'a>(
    app: &'a HorizonApp,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) -> Vec<MinimapWorkspaceLabel<'a>> {
    let mut labels = Vec::new();

    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }

        let is_active =
            app.board.active_workspace == Some(workspace.id) || scope == MinimapScope::Workspace(workspace.id);
        let workspace_rect =
            workspace_minimap_screen_rect(origin, model, workspace.id, workspace.position, workspace_bounds);
        let title_strip_rect = minimap_workspace_title_strip_rect(workspace_rect, model.scale_y);

        labels.push(MinimapWorkspaceLabel {
            name: &workspace.name,
            color: theme::workspace_accent(workspace.color_idx),
            is_active,
            workspace_rect,
            title_strip_rect,
        });
    }

    labels
}

/// Active-first ordering is load-bearing: the active workspace's label paints
/// before any `occupied` rects exist, so a collision can never suppress it.
fn minimap_workspace_label_order(left: &MinimapWorkspaceLabel<'_>, right: &MinimapWorkspaceLabel<'_>) -> Ordering {
    right
        .is_active
        .cmp(&left.is_active)
        .then_with(|| {
            let left_area = left.workspace_rect.width() * left.workspace_rect.height();
            let right_area = right.workspace_rect.width() * right.workspace_rect.height();
            right_area.total_cmp(&left_area)
        })
        .then_with(|| left.workspace_rect.min.y.total_cmp(&right.workspace_rect.min.y))
        .then_with(|| left.workspace_rect.min.x.total_cmp(&right.workspace_rect.min.x))
}

fn minimap_workspace_title_strip_rect(workspace_rect: Rect, scale_y: f32) -> Option<Rect> {
    const MIN_LABEL_WIDTH: f32 = 34.0;
    const MIN_STRIP_HEIGHT: f32 = 10.0;

    if workspace_rect.width() < MIN_LABEL_WIDTH || workspace_rect.height() < MIN_STRIP_HEIGHT {
        return None;
    }

    let desired_height = (WS_TITLE_HEIGHT * scale_y).clamp(10.0, 18.0);
    let strip_height = desired_height.min(workspace_rect.height() - 2.0);
    if strip_height < MIN_STRIP_HEIGHT {
        return None;
    }

    Some(Rect::from_min_max(
        workspace_rect.min,
        Pos2::new(workspace_rect.max.x, workspace_rect.min.y + strip_height),
    ))
}

const HORIZ_LABEL_PAD_X: f32 = 5.0;
const HORIZ_MIN_TEXT_WIDTH: f32 = 12.0;
const VERT_PAD: f32 = 3.0;
const VERT_MIN_HEIGHT: f32 = 16.0;
const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 6.5..=10.5;
/// Both label paths clip glyphs to their badge shrunk by 1px on each side.
const BADGE_CLIP_MARGIN: f32 = 2.0;

struct HorizontalLabelLayout {
    strip: Rect,
    badge_height: f32,
    galley: Arc<Galley>,
}

struct VerticalLabelLayout {
    galley: Arc<Galley>,
}

/// A horizontal label wins when it shows the whole name; otherwise the
/// orientation whose measured galley shows more glyphs wins, ties going to
/// horizontal. If the preferred orientation cannot be placed without
/// colliding, the other one is tried.
fn paint_minimap_workspace_label(painter: &Painter, label: &MinimapWorkspaceLabel<'_>, occupied: &mut Vec<Rect>) {
    if label.name.is_empty() {
        return;
    }

    let horizontal = horizontal_label_layout(painter, label);
    let vertical = vertical_label_layout(painter, label);

    let horizontal_first = match (&horizontal, &vertical) {
        (Some(horiz), Some(vert)) => !horiz.galley.elided || glyph_count(&horiz.galley) >= glyph_count(&vert.galley),
        (Some(_), None) => true,
        _ => false,
    };

    if horizontal_first {
        if let Some(horiz) = &horizontal
            && try_paint_horizontal_label(painter, label, horiz, occupied)
        {
            return;
        }
        if let Some(vert) = &vertical {
            try_paint_vertical_label(painter, label, vert, occupied);
        }
    } else {
        if let Some(vert) = &vertical
            && try_paint_vertical_label(painter, label, vert, occupied)
        {
            return;
        }
        if let Some(horiz) = &horizontal {
            try_paint_horizontal_label(painter, label, horiz, occupied);
        }
    }
}

fn glyph_count(galley: &Galley) -> usize {
    galley.rows.iter().map(|row| row.glyphs.len()).sum()
}

/// An elided galley that kept nothing but the overflow ellipsis carries no
/// information; treat it as unpaintable.
fn has_visible_glyphs(galley: &Galley) -> bool {
    if galley.elided {
        glyph_count(galley) > 1
    } else {
        glyph_count(galley) > 0
    }
}

/// Largest font in [`FONT_SIZE_RANGE`] whose real row height fits `max_row_height`.
fn font_fitting_row_height(painter: &Painter, max_row_height: f32) -> Option<FontId> {
    let (min_size, max_size) = (*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end());
    let mut size = max_size;
    while size >= min_size {
        let font = FontId::proportional(size);
        let row_height = painter.ctx().fonts_mut(|fonts| fonts.row_height(&font));
        if row_height <= max_row_height {
            return Some(font);
        }
        size -= 0.5;
    }
    None
}

fn horizontal_label_layout(painter: &Painter, label: &MinimapWorkspaceLabel<'_>) -> Option<HorizontalLabelLayout> {
    let strip = label.title_strip_rect?;
    let badge_height = (strip.height() - 2.0).clamp(10.0, 16.0);
    let max_text_width = strip.width() - HORIZ_LABEL_PAD_X * 2.0 - 4.0;
    if max_text_width < HORIZ_MIN_TEXT_WIDTH {
        return None;
    }

    let font = font_fitting_row_height(painter, badge_height - BADGE_CLIP_MARGIN)?;
    let galley = painter.layout_job(single_line_label_job(
        label.name,
        &font,
        label_text_color(label.is_active),
        max_text_width,
    ));
    has_visible_glyphs(&galley).then_some(HorizontalLabelLayout {
        strip,
        badge_height,
        galley,
    })
}

fn vertical_label_layout(painter: &Painter, label: &MinimapWorkspaceLabel<'_>) -> Option<VerticalLabelLayout> {
    let workspace_rect = label.workspace_rect;
    if workspace_rect.height() < VERT_MIN_HEIGHT {
        return None;
    }

    // Rotated a quarter turn, the line height spans the column's width: the
    // badge caps at width - 2 and the paint clips 1px inside it on each side.
    let font = font_fitting_row_height(painter, workspace_rect.width() - 2.0 - BADGE_CLIP_MARGIN)?;
    let max_text_length = workspace_rect.height() - 2.0 - VERT_PAD * 2.0;
    let galley = painter.layout_job(single_line_label_job(
        label.name,
        &font,
        label_text_color(label.is_active),
        max_text_length,
    ));
    has_visible_glyphs(&galley).then_some(VerticalLabelLayout { galley })
}

fn try_paint_horizontal_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    layout: &HorizontalLabelLayout,
    occupied: &mut Vec<Rect>,
) -> bool {
    let galley_size = layout.galley.size();
    let badge_width = (galley_size.x + HORIZ_LABEL_PAD_X * 2.0).min(layout.strip.width() - 2.0);
    let base_rect = Rect::from_min_size(
        Pos2::new(
            layout.strip.min.x + 1.0,
            layout.strip.center().y - layout.badge_height * 0.5,
        ),
        Vec2::new(badge_width, layout.badge_height),
    );

    let Some(badge_rect) = resolve_label_rect(base_rect, label.workspace_rect, occupied) else {
        return false;
    };

    paint_label_badge(painter, badge_rect, label.color, label.is_active);

    let text_pos = Pos2::new(
        badge_rect.min.x + HORIZ_LABEL_PAD_X,
        badge_rect.center().y - galley_size.y * 0.5,
    );
    painter
        .with_clip_rect(badge_rect.shrink2(Vec2::new(HORIZ_LABEL_PAD_X - 1.0, 1.0)))
        .galley(text_pos, Arc::clone(&layout.galley), Color32::TRANSPARENT);

    occupied.push(badge_rect.expand(1.0));
    true
}

fn try_paint_vertical_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    layout: &VerticalLabelLayout,
    occupied: &mut Vec<Rect>,
) -> bool {
    let Some(base_rect) = vertical_label_badge_rect(label.workspace_rect, layout.galley.size()) else {
        return false;
    };
    let Some(badge_rect) = resolve_label_rect(base_rect, label.workspace_rect, occupied) else {
        return false;
    };

    paint_label_badge(painter, badge_rect, label.color, label.is_active);

    // Rotate the single-line galley a quarter turn clockwise about its center
    // so the name reads top-to-bottom like a book spine, centered in the badge.
    let text_pos = badge_rect.center() - layout.galley.size() * 0.5;
    painter.with_clip_rect(badge_rect.shrink(1.0)).add(
        TextShape::new(text_pos, Arc::clone(&layout.galley), Color32::TRANSPARENT)
            .with_angle_and_anchor(FRAC_PI_2, Align2::CENTER_CENTER),
    );

    occupied.push(badge_rect.expand(1.0));
    true
}

fn vertical_label_badge_rect(workspace_rect: Rect, galley_size: Vec2) -> Option<Rect> {
    let badge_width = (galley_size.y + VERT_PAD * 2.0).min(workspace_rect.width() - 2.0);
    let badge_height = (galley_size.x + VERT_PAD * 2.0).min(workspace_rect.height() - 2.0);
    if badge_width < 4.0 || badge_height < 4.0 {
        return None;
    }

    Some(Rect::from_min_size(
        Pos2::new(workspace_rect.min.x + 1.0, workspace_rect.min.y + 1.0),
        Vec2::new(badge_width, badge_height),
    ))
}

/// Slide the badge down in steps until it stops colliding with earlier labels.
/// Offsets clamp to the workspace's bottom edge; once a candidate has sat at
/// the clamp, every remaining offset would repeat it.
fn resolve_label_rect(base_rect: Rect, workspace_rect: Rect, occupied: &[Rect]) -> Option<Rect> {
    const Y_OFFSETS: [f32; 4] = [0.0, 6.0, 12.0, 18.0];

    let max_top = (workspace_rect.max.y - base_rect.height() - 1.0).max(base_rect.min.y);
    for y_offset in Y_OFFSETS {
        let desired_top = base_rect.min.y + y_offset;
        let candidate = Rect::from_min_size(Pos2::new(base_rect.min.x, desired_top.min(max_top)), base_rect.size());
        if !occupied.iter().any(|rect| rect.intersects(candidate)) {
            return Some(candidate);
        }
        if desired_top >= max_top {
            break;
        }
    }

    None
}

fn paint_label_badge(painter: &Painter, rect: Rect, color: Color32, is_active: bool) {
    let fill = theme::alpha(
        theme::blend(theme::BG_ELEVATED(), color, if is_active { 0.24 } else { 0.14 }),
        236,
    );
    let stroke = Stroke::new(1.0_f32, theme::alpha(color, if is_active { 210 } else { 140 }));
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    painter.rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Outside);
}

fn label_text_color(is_active: bool) -> Color32 {
    if is_active {
        theme::FG()
    } else {
        theme::alpha(theme::FG_SOFT(), 240)
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};

    use super::{VERT_PAD, resolve_label_rect, vertical_label_badge_rect};

    #[test]
    fn vertical_label_badge_wraps_rotated_galley_with_padding() {
        let workspace_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(24.0, 120.0));
        let galley_size = Vec2::new(60.0, 10.0);

        let badge_rect = vertical_label_badge_rect(workspace_rect, galley_size);

        // Rotated a quarter turn: galley height becomes badge width, galley
        // length becomes badge height, each padded on both sides.
        let expected_size = Vec2::new(galley_size.y + VERT_PAD * 2.0, galley_size.x + VERT_PAD * 2.0);
        assert_eq!(
            badge_rect,
            Some(Rect::from_min_size(Pos2::new(11.0, 21.0), expected_size))
        );
    }

    #[test]
    fn vertical_label_badge_clamps_to_workspace_size() {
        let workspace_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(12.0, 40.0));

        let badge_rect = vertical_label_badge_rect(workspace_rect, Vec2::new(200.0, 30.0));

        assert_eq!(
            badge_rect,
            Some(Rect::from_min_size(Pos2::new(1.0, 1.0), Vec2::new(10.0, 38.0)))
        );
    }

    #[test]
    fn resolve_label_rect_slides_past_occupied_space() {
        let workspace_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 100.0));
        let base_rect = Rect::from_min_size(Pos2::new(1.0, 1.0), Vec2::new(40.0, 10.0));
        let occupied = vec![base_rect.expand(1.0)];

        let resolved = resolve_label_rect(base_rect, workspace_rect, &occupied).expect("resolved rect");

        assert_eq!(resolved.min, Pos2::new(1.0, 13.0));
    }

    #[test]
    fn resolve_label_rect_gives_up_when_everything_collides() {
        let workspace_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 100.0));
        let base_rect = Rect::from_min_size(Pos2::new(1.0, 1.0), Vec2::new(40.0, 10.0));
        let occupied = vec![Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 100.0))];

        assert_eq!(resolve_label_rect(base_rect, workspace_rect, &occupied), None);
    }

    #[test]
    fn resolve_label_rect_stops_retrying_once_offsets_clamp() {
        // A workspace barely taller than the badge clamps every offset to the
        // same top; the search must terminate without duplicate candidates.
        let workspace_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(60.0, 12.0));
        let base_rect = Rect::from_min_size(Pos2::new(1.0, 1.0), Vec2::new(40.0, 10.0));
        let occupied = vec![workspace_rect];

        assert_eq!(resolve_label_rect(base_rect, workspace_rect, &occupied), None);
    }
}
