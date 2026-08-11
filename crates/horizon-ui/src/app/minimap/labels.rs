use std::{cmp::Ordering, collections::HashMap, f32::consts::FRAC_PI_2};

use egui::{
    Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
    epaint::TextShape,
    text::{LayoutJob, TextFormat, TextWrapping},
};
use horizon_core::WorkspaceId;

use crate::theme;

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

fn paint_minimap_workspace_label(painter: &Painter, label: &MinimapWorkspaceLabel<'_>, occupied: &mut Vec<Rect>) {
    if label.name.is_empty() {
        return;
    }

    let name_chars = label.name.chars().count();
    let horiz_cap = label.title_strip_rect.map_or(0, estimated_horizontal_chars);
    let vert_cap = estimated_vertical_chars(label.workspace_rect);

    if prefers_horizontal_label(name_chars, horiz_cap, vert_cap) {
        if let Some(strip) = label.title_strip_rect
            && try_paint_horizontal_label(painter, label, strip, occupied)
        {
            return;
        }
        try_paint_vertical_label(painter, label, occupied);
    } else {
        if try_paint_vertical_label(painter, label, occupied) {
            return;
        }
        if let Some(strip) = label.title_strip_rect {
            try_paint_horizontal_label(painter, label, strip, occupied);
        }
    }
}

/// A horizontal label wins whenever it can show the whole name; otherwise the
/// orientation that fits more characters wins, with ties going to horizontal.
fn prefers_horizontal_label(name_chars: usize, horiz_cap: usize, vert_cap: usize) -> bool {
    horiz_cap >= name_chars || horiz_cap >= vert_cap
}

const HORIZ_LABEL_PAD_X: f32 = 5.0;
const HORIZ_MIN_TEXT_WIDTH: f32 = 12.0;
const VERT_PAD: f32 = 3.0;
const VERT_MIN_HEIGHT: f32 = 16.0;
const VERT_MIN_WIDTH: f32 = 8.0;
/// Approximate single-line galley height per point of font size for the
/// default proportional font; used to pick a font that fits the column width.
const LINE_HEIGHT_RATIO: f32 = 1.3;
/// Approximate glyph advance per point of font size, shared by both
/// orientation capacity estimates.
const AVG_CHAR_WIDTH_RATIO: f32 = 0.65;

fn horiz_label_metrics(strip: Rect) -> (f32, f32, f32) {
    let badge_height = (strip.height() - 2.0).clamp(10.0, 16.0);
    let font_size = (badge_height - 2.0).clamp(7.5, 10.5);
    let max_text_width = strip.width() - HORIZ_LABEL_PAD_X * 2.0 - 4.0;
    (badge_height, font_size, max_text_width)
}

fn estimated_horizontal_chars(strip: Rect) -> usize {
    let (_, font_size, max_text_width) = horiz_label_metrics(strip);
    if max_text_width < HORIZ_MIN_TEXT_WIDTH {
        return 0;
    }
    let avg_char_width = font_size * AVG_CHAR_WIDTH_RATIO;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (max_text_width / avg_char_width).floor() as usize
    }
}

/// Font size for a rotated label, chosen so one line height (plus padding)
/// spans the workspace column's width.
fn vert_label_font_size(workspace_rect: Rect) -> f32 {
    ((workspace_rect.width() - VERT_PAD * 2.0 - 2.0) / LINE_HEIGHT_RATIO).clamp(6.5, 10.5)
}

fn estimated_vertical_chars(workspace_rect: Rect) -> usize {
    if workspace_rect.height() < VERT_MIN_HEIGHT || workspace_rect.width() < VERT_MIN_WIDTH {
        return 0;
    }
    let font_size = vert_label_font_size(workspace_rect);
    let available_height = workspace_rect.height() - 2.0 - VERT_PAD * 2.0;
    let avg_char_width = font_size * AVG_CHAR_WIDTH_RATIO;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (available_height / avg_char_width).floor().max(0.0) as usize
    }
}

fn try_paint_horizontal_label(
    painter: &Painter,
    label: &MinimapWorkspaceLabel<'_>,
    strip: Rect,
    occupied: &mut Vec<Rect>,
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

    let Some(badge_rect) = place_minimap_label_rect(base_rect, label.workspace_rect, occupied, label.is_active) else {
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

fn try_paint_vertical_label(painter: &Painter, label: &MinimapWorkspaceLabel<'_>, occupied: &mut Vec<Rect>) -> bool {
    let workspace_rect = label.workspace_rect;
    if workspace_rect.height() < VERT_MIN_HEIGHT || workspace_rect.width() < VERT_MIN_WIDTH {
        return false;
    }

    let font_size = vert_label_font_size(workspace_rect);
    let max_text_length = workspace_rect.height() - 2.0 - VERT_PAD * 2.0;
    if max_text_length < font_size {
        return false;
    }

    let font = FontId::proportional(font_size);
    let text_color = label_text_color(label.is_active);
    let galley = painter.layout_job(single_line_label_job(label.name, &font, text_color, max_text_length));

    let Some(badge_rect) = vertical_label_badge_rect(workspace_rect, galley.size()) else {
        return false;
    };
    if occupied.iter().any(|rect| rect.intersects(badge_rect)) && !label.is_active {
        return false;
    }

    paint_label_badge(painter, badge_rect, label.color, label.is_active);

    // Rotate the single-line galley a quarter turn clockwise so the name reads
    // top-to-bottom like a book spine. `pos` stays the galley origin, so after
    // rotation the line height extends to its left: anchor the origin half a
    // line height right of the badge centerline, at the badge's padded top.
    let text_pos = Pos2::new(
        badge_rect.center().x + galley.size().y * 0.5,
        badge_rect.min.y + VERT_PAD,
    );
    painter
        .with_clip_rect(badge_rect.shrink(1.0))
        .add(TextShape::new(text_pos, galley, text_color).with_angle(FRAC_PI_2));

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

fn place_minimap_label_rect(base_rect: Rect, workspace_rect: Rect, occupied: &[Rect], is_active: bool) -> Option<Rect> {
    const Y_OFFSETS: [f32; 4] = [0.0, 6.0, 12.0, 18.0];

    for y_offset in Y_OFFSETS {
        let max_top = (workspace_rect.max.y - base_rect.height() - 1.0).max(base_rect.min.y);
        let top = (base_rect.min.y + y_offset).min(max_top);
        let candidate = Rect::from_min_size(Pos2::new(base_rect.min.x, top), base_rect.size());
        if !occupied.iter().any(|rect| rect.intersects(candidate)) {
            return Some(candidate);
        }
    }

    is_active.then_some(base_rect)
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect, Vec2};

    use super::{VERT_PAD, estimated_vertical_chars, prefers_horizontal_label, vertical_label_badge_rect};

    #[test]
    fn vertical_label_badge_rect_skips_sub_two_pixel_workspaces() {
        let workspace_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(1.5, 40.0));

        let badge_rect = vertical_label_badge_rect(workspace_rect, Vec2::new(30.0, 9.0));

        assert_eq!(badge_rect, None);
    }

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
    fn horizontal_label_wins_when_it_fits_the_whole_name() {
        assert!(prefers_horizontal_label(8, 8, 40));
        assert!(prefers_horizontal_label(8, 10, 6));
        assert!(!prefers_horizontal_label(12, 3, 18));
    }

    #[test]
    fn narrow_columns_estimate_more_rotated_chars_than_wide_short_strips() {
        let tall_narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(22.0, 130.0));
        let chars = estimated_vertical_chars(tall_narrow);

        // A 130px tall column comfortably fits a double-digit character count
        // once the label is rotated instead of stacked one glyph per line.
        assert!(chars >= 15, "expected >= 15 rotated chars, got {chars}");

        assert_eq!(
            estimated_vertical_chars(Rect::from_min_size(Pos2::ZERO, Vec2::new(22.0, 12.0))),
            0
        );
        assert_eq!(
            estimated_vertical_chars(Rect::from_min_size(Pos2::ZERO, Vec2::new(6.0, 130.0))),
            0
        );
    }
}
