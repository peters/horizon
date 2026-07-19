use std::{cmp::Ordering, collections::HashMap};

use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
    text::{LayoutJob, TextFormat, TextWrapping},
};
use horizon_core::WorkspaceId;

use crate::theme;

use super::{
    HorizonApp, MinimapModel, MinimapScope, WS_TITLE_HEIGHT, scope_includes_workspace, workspace_minimap_rect,
};

struct MinimapWorkspaceLabel<'a> {
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
    exclusions: &[Rect],
) -> Vec<Rect> {
    let mut labels = collect_minimap_workspace_labels(app, origin, model, workspace_bounds, scope);
    labels.sort_by(minimap_workspace_label_order);

    let exclusion_count = exclusions.len();
    let mut occupied = Vec::with_capacity(exclusion_count + labels.len());
    occupied.extend_from_slice(exclusions);
    for label in labels {
        paint_minimap_workspace_label(painter, &label, &mut occupied, exclusion_count);
    }
    occupied.into_iter().skip(exclusion_count).collect()
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
        let workspace_rect = workspace_minimap_rect(workspace.id, workspace.position, origin, model, workspace_bounds);
        let title_strip_rect = minimap_workspace_title_strip_rect(workspace_rect, model.scale_y);

        let (r, g, b) = workspace.accent();
        labels.push(MinimapWorkspaceLabel {
            name: &workspace.name,
            color: Color32::from_rgb(r, g, b),
            is_active,
            workspace_rect,
            title_strip_rect,
        });
    }

    labels
}

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

fn paint_minimap_workspace_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    occupied: &mut Vec<Rect>,
    protected_count: usize,
) {
    let horiz_cap = label.title_strip_rect.map_or(0, estimated_horizontal_chars);
    let vert_cap = estimated_vertical_chars(label.workspace_rect);

    if horiz_cap >= vert_cap {
        if let Some(strip) = label.title_strip_rect
            && try_paint_horizontal_label(painter, label, strip, occupied, protected_count)
        {
            return;
        }
        try_paint_vertical_label(painter, label, occupied, protected_count);
    } else {
        if try_paint_vertical_label(painter, label, occupied, protected_count) {
            return;
        }
        if let Some(strip) = label.title_strip_rect {
            try_paint_horizontal_label(painter, label, strip, occupied, protected_count);
        }
    }
}

const HORIZ_LABEL_PAD_X: f32 = 5.0;
const HORIZ_MIN_TEXT_WIDTH: f32 = 12.0;
const VERT_PAD: f32 = 3.0;
const VERT_MIN_HEIGHT: f32 = 16.0;

fn horiz_label_metrics(strip: Rect) -> (f32, f32, f32) {
    let badge_height = (strip.height() - 2.0).clamp(10.0, 16.0);
    let font_size = (badge_height - 2.0).clamp(7.5, 10.5);
    let max_text_width = strip.width() - HORIZ_LABEL_PAD_X * 2.0 - 4.0;
    (badge_height, font_size, max_text_width)
}

fn vert_label_metrics(workspace_rect: Rect) -> (f32, f32, f32) {
    let font_size = (workspace_rect.width() * 0.35).clamp(6.0, 9.0);
    let line_height = font_size + 2.0;
    let available_height = workspace_rect.height() - 2.0;
    (font_size, line_height, available_height)
}

fn estimated_horizontal_chars(strip: Rect) -> usize {
    let (_, font_size, max_text_width) = horiz_label_metrics(strip);
    if max_text_width < HORIZ_MIN_TEXT_WIDTH {
        return 0;
    }
    let avg_char_width = font_size * 0.65;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (max_text_width / avg_char_width).floor() as usize
    }
}

fn estimated_vertical_chars(workspace_rect: Rect) -> usize {
    if workspace_rect.height() < VERT_MIN_HEIGHT {
        return 0;
    }
    let (_, line_height, available_height) = vert_label_metrics(workspace_rect);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        ((available_height - VERT_PAD * 2.0) / line_height).floor().max(0.0) as usize
    }
}

fn try_paint_horizontal_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    strip: Rect,
    occupied: &mut Vec<Rect>,
    protected_count: usize,
) -> bool {
    let (badge_height, font_size, max_text_width) = horiz_label_metrics(strip);
    let font = FontId::proportional(font_size);
    if max_text_width < HORIZ_MIN_TEXT_WIDTH {
        return false;
    }

    let text_color = label_text_color(label.is_active);
    let galley = painter.layout_job(single_line_label_job(label.name, &font, text_color, max_text_width));
    let badge_width = (galley.size().x + HORIZ_LABEL_PAD_X * 2.0).min(strip.width() - 2.0);
    let base_rect = Rect::from_min_size(
        Pos2::new(strip.min.x + 1.0, strip.center().y - badge_height * 0.5),
        Vec2::new(badge_width, badge_height),
    );

    let Some(badge_rect) = place_minimap_label_rect(
        base_rect,
        label.workspace_rect,
        occupied,
        protected_count,
        label.is_active,
    ) else {
        return false;
    };

    paint_label_badge(painter, badge_rect, label.color, label.is_active);

    let text_pos = Pos2::new(
        badge_rect.min.x + HORIZ_LABEL_PAD_X,
        badge_rect.center().y - galley.size().y * 0.5,
    );
    painter
        .with_clip_rect(badge_rect.shrink2(Vec2::new(HORIZ_LABEL_PAD_X - 1.0, 1.0)))
        .galley(text_pos, galley, Color32::TRANSPARENT);

    occupied.push(badge_rect.expand(1.0));
    true
}

fn try_paint_vertical_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    occupied: &mut Vec<Rect>,
    protected_count: usize,
) -> bool {
    if label.workspace_rect.height() < VERT_MIN_HEIGHT {
        return false;
    }

    let (font_size, line_height, available_height) = vert_label_metrics(label.workspace_rect);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let max_chars = ((available_height - VERT_PAD * 2.0) / line_height).floor().max(0.0) as usize;

    // Single-pass: take up to max_chars+1 to detect truncation without a separate .count()
    let mut visible: Vec<char> = label.name.chars().take(max_chars.saturating_add(1)).collect();
    let truncated = visible.len() > max_chars;
    visible.truncate(max_chars);
    if visible.is_empty() {
        return false;
    }

    #[allow(clippy::cast_precision_loss)]
    let badge_height = (visible.len() as f32 * line_height + VERT_PAD * 2.0).min(available_height);
    let Some(badge_rect) = vertical_label_badge_rect(label.workspace_rect, font_size, badge_height) else {
        return false;
    };

    let overlaps_protected = occupied[..protected_count]
        .iter()
        .any(|rect| rect.intersects(badge_rect));
    let overlaps_label = occupied[protected_count..]
        .iter()
        .any(|rect| rect.intersects(badge_rect));
    if overlaps_protected || (overlaps_label && !label.is_active) {
        return false;
    }

    paint_label_badge(painter, badge_rect, label.color, label.is_active);

    let font = FontId::proportional(font_size);
    let text_color = label_text_color(label.is_active);
    let clipped = painter.with_clip_rect(badge_rect.shrink(1.0));
    let mut buf = [0u8; 4];
    for (i, &ch) in visible.iter().enumerate() {
        let is_last = i + 1 == visible.len() && truncated;
        let display = if is_last { "\u{2026}" } else { ch.encode_utf8(&mut buf) };
        #[allow(clippy::cast_precision_loss)]
        let char_y = badge_rect.min.y + VERT_PAD + (i as f32) * line_height + line_height * 0.5;
        clipped.text(
            Pos2::new(badge_rect.center().x, char_y),
            Align2::CENTER_CENTER,
            display,
            font.clone(),
            text_color,
        );
    }

    occupied.push(badge_rect.expand(1.0));
    true
}

fn vertical_label_badge_rect(workspace_rect: Rect, font_size: f32, badge_height: f32) -> Option<Rect> {
    let badge_width = (font_size + VERT_PAD * 2.0).min(workspace_rect.width() - 2.0);
    if badge_width <= 0.0 || badge_height <= 0.0 {
        return None;
    }

    Some(Rect::from_min_size(
        Pos2::new(workspace_rect.min.x + 1.0, workspace_rect.min.y + 1.0),
        Vec2::new(badge_width, badge_height),
    ))
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

fn single_line_label_job(text: &str, font: &FontId, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    job
}

fn place_minimap_label_rect(
    base_rect: Rect,
    workspace_rect: Rect,
    occupied: &[Rect],
    protected_count: usize,
    is_active: bool,
) -> Option<Rect> {
    let candidates = minimap_label_candidates(base_rect, workspace_rect);
    if let Some(candidate) = candidates
        .iter()
        .find(|candidate| occupied.iter().all(|rect| !rect.intersects(**candidate)))
    {
        return Some(*candidate);
    }
    is_active.then(|| {
        candidates.into_iter().find(|candidate| {
            occupied[..protected_count]
                .iter()
                .all(|rect| !rect.intersects(*candidate))
        })
    })?
}

fn minimap_label_candidates(base_rect: Rect, workspace_rect: Rect) -> [Rect; 4] {
    const Y_OFFSETS: [f32; 4] = [0.0, 6.0, 12.0, 18.0];

    Y_OFFSETS.map(|y_offset| {
        let max_top = (workspace_rect.max.y - base_rect.height() - 1.0).max(base_rect.min.y);
        let top = (base_rect.min.y + y_offset).min(max_top);
        Rect::from_min_size(Pos2::new(base_rect.min.x, top), base_rect.size())
    })
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};

    use super::{place_minimap_label_rect, vertical_label_badge_rect};

    #[test]
    fn vertical_label_badge_rect_skips_sub_two_pixel_workspaces() {
        let workspace_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(1.5, 40.0));

        let badge_rect = vertical_label_badge_rect(workspace_rect, 6.0, 24.0);

        assert_eq!(badge_rect, None);
    }

    #[test]
    fn active_label_fallback_avoids_protected_cue_geometry() {
        let workspace_rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(60.0, 50.0));
        let base_rect = Rect::from_min_size(Pos2::new(12.0, 12.0), Vec2::new(36.0, 10.0));
        let protected = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(60.0, 11.0));
        let ordinary_label = workspace_rect;

        let placed = place_minimap_label_rect(base_rect, workspace_rect, &[protected, ordinary_label], 1, true)
            .expect("active label should find a protected-safe fallback");

        assert!(!placed.intersects(protected));
        assert!(placed.intersects(ordinary_label));
        assert_eq!(
            place_minimap_label_rect(base_rect, workspace_rect, &[protected, ordinary_label], 1, false,),
            None
        );
    }
}
