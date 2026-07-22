use std::{cmp::Ordering, collections::HashMap};

use egui::{
    Align2, Color32, Context, CornerRadius, FontId, Id, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2,
    text::{LayoutJob, TextFormat, TextWrapping},
};
use horizon_core::{PanelId, WorkspaceId};

use crate::theme;

use super::{HorizonApp, MINIMAP_MARGIN, MINIMAP_PAD, WS_BG_PAD, WS_EMPTY_SIZE, WS_TITLE_HEIGHT};

mod interaction;

use interaction::{minimap_panels_in_paint_order, render_scoped_minimap, scope_includes_workspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapScope {
    Attached,
    Workspace(WorkspaceId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapHitTarget {
    Panel {
        panel_id: PanelId,
        workspace_id: WorkspaceId,
    },
    Workspace(WorkspaceId),
}

impl MinimapHitTarget {
    fn workspace_id(self) -> WorkspaceId {
        match self {
            Self::Panel { workspace_id, .. } | Self::Workspace(workspace_id) => workspace_id,
        }
    }
}

struct MinimapModel {
    content_min: [f32; 2],
    scale_x: f32,
    scale_y: f32,
    outer_size: Vec2,
    view_min: Pos2,
    view_max: Pos2,
}

struct MinimapWorkspaceLabel<'a> {
    name: &'a str,
    color: Color32,
    is_active: bool,
    workspace_rect: Rect,
    title_strip_rect: Option<Rect>,
}

impl HorizonApp {
    pub(super) fn render_minimap(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    ) -> f32 {
        render_scoped_minimap(
            self,
            ctx,
            workspace_bounds,
            self.canvas_rect(ctx),
            MinimapScope::Attached,
            Id::new("minimap_overlay"),
        )
    }

    pub(super) fn render_workspace_minimap(
        &mut self,
        ctx: &Context,
        workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
        workspace_id: WorkspaceId,
        canvas_rect: Rect,
        overlay_id: Id,
    ) -> f32 {
        render_scoped_minimap(
            self,
            ctx,
            workspace_bounds,
            canvas_rect,
            MinimapScope::Workspace(workspace_id),
            overlay_id,
        )
    }
}

fn minimap_model(
    app: &HorizonApp,
    canvas_rect: Rect,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) -> Option<MinimapModel> {
    let (content_min, content_max) = workspace_content_bounds(app, workspace_bounds, scope)?;
    let view_min = app.screen_to_canvas(canvas_rect, canvas_rect.min);
    let view_max = app.screen_to_canvas(canvas_rect, canvas_rect.max);

    let content_w = content_max[0] - content_min[0];
    let content_h = content_max[1] - content_min[1];
    if content_w < 1.0 || content_h < 1.0 {
        return None;
    }

    let overlays = &app.template_config.overlays;
    let map_w = overlays.minimap_width.max(120.0);
    let map_h = overlays.minimap_height.max(120.0);

    Some(MinimapModel {
        content_min,
        scale_x: map_w / content_w,
        scale_y: map_h / content_h,
        outer_size: Vec2::new(map_w + MINIMAP_PAD * 2.0, map_h + MINIMAP_PAD * 2.0),
        view_min,
        view_max,
    })
}

fn paint_minimap_contents(
    app: &HorizonApp,
    painter: &Painter,
    rect: Rect,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
    hovered: Option<MinimapHitTarget>,
) {
    painter.rect_filled(rect, CornerRadius::same(8), theme::alpha(theme::BG_ELEVATED(), 220));
    painter.rect_stroke(
        rect,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, theme::alpha(theme::BORDER_SUBTLE(), 180)),
        StrokeKind::Outside,
    );

    let origin = rect.min;
    let hovered_workspace = hovered.map(MinimapHitTarget::workspace_id);
    paint_minimap_workspaces(app, painter, origin, model, workspace_bounds, scope, hovered_workspace);
    paint_minimap_panels(app, painter, origin, model, scope);
    paint_minimap_workspace_labels(app, painter, origin, model, workspace_bounds, scope);
    paint_minimap_viewport(painter, origin, model);
}

fn paint_minimap_workspaces(
    app: &HorizonApp,
    painter: &Painter,
    origin: Pos2,
    model: &MinimapModel,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
    hovered_workspace: Option<WorkspaceId>,
) {
    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }
        let (r, g, b) = workspace.accent();
        let workspace_color = Color32::from_rgb(r, g, b);
        let is_active =
            app.board.active_workspace == Some(workspace.id) || scope == MinimapScope::Workspace(workspace.id);
        let is_hovered = hovered_workspace == Some(workspace.id);
        let workspace_rect =
            workspace_minimap_screen_rect(origin, model, workspace.id, workspace.position, workspace_bounds);

        let fill_alpha = if is_active {
            60
        } else if is_hovered {
            34
        } else {
            22
        };
        let stroke_alpha = if is_active {
            210
        } else if is_hovered {
            180
        } else {
            80
        };
        painter.rect_filled(
            workspace_rect,
            CornerRadius::same(2),
            theme::alpha(workspace_color, fill_alpha),
        );
        painter.rect_stroke(
            workspace_rect,
            CornerRadius::same(2),
            Stroke::new(0.8_f32, theme::alpha(workspace_color, stroke_alpha)),
            StrokeKind::Outside,
        );

        if is_active && scope == MinimapScope::Attached {
            painter.rect_stroke(
                workspace_rect.expand(3.0),
                CornerRadius::same(4),
                Stroke::new(2.0_f32, theme::alpha(theme::ACCENT(), 160)),
                StrokeKind::Outside,
            );
        }
    }
}

fn paint_minimap_workspace_labels(
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
    let horiz_cap = label.title_strip_rect.map_or(0, estimated_horizontal_chars);
    let vert_cap = estimated_vertical_chars(label.workspace_rect);

    if horiz_cap >= vert_cap {
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

    if occupied.iter().any(|r| r.intersects(badge_rect)) && !label.is_active {
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

fn paint_minimap_panels(app: &HorizonApp, painter: &Painter, origin: Pos2, model: &MinimapModel, scope: MinimapScope) {
    for panel in minimap_panels_in_paint_order(app, scope) {
        let panel_rect = panel_minimap_screen_rect(origin, model, panel.layout.position, panel.layout.size);
        let workspace_color = app
            .board
            .workspace(panel.workspace_id)
            .map_or(theme::ACCENT(), |workspace| {
                let (r, g, b) = workspace.accent();
                Color32::from_rgb(r, g, b)
            });
        let is_focused = app.board.focused == Some(panel.id);

        painter.rect_filled(
            panel_rect,
            CornerRadius::same(1),
            theme::alpha(workspace_color, if is_focused { 120 } else { 70 }),
        );
        if is_focused {
            painter.rect_stroke(
                panel_rect,
                CornerRadius::same(1),
                Stroke::new(1.0_f32, theme::alpha(theme::FG(), 220)),
                StrokeKind::Outside,
            );
        }
    }
}

fn workspace_content_bounds(
    app: &HorizonApp,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
    scope: MinimapScope,
) -> Option<([f32; 2], [f32; 2])> {
    let mut content_min = [f32::MAX, f32::MAX];
    let mut content_max = [f32::MIN, f32::MIN];
    let mut has_content = false;

    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }
        let (workspace_min, workspace_max) =
            workspace_minimap_bounds(workspace.id, workspace_bounds).unwrap_or_else(|| {
                let pos = workspace.position;
                (pos, [pos[0] + WS_EMPTY_SIZE[0], pos[1] + WS_EMPTY_SIZE[1]])
            });
        content_min[0] = content_min[0].min(workspace_min[0]);
        content_min[1] = content_min[1].min(workspace_min[1]);
        content_max[0] = content_max[0].max(workspace_max[0]);
        content_max[1] = content_max[1].max(workspace_max[1]);
        has_content = true;
    }

    has_content.then_some((content_min, content_max))
}

fn workspace_minimap_bounds(
    workspace_id: WorkspaceId,
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Option<([f32; 2], [f32; 2])> {
    workspace_bounds
        .get(&workspace_id)
        .copied()
        .map(|(workspace_min, workspace_max)| {
            (
                [
                    workspace_min[0] - WS_BG_PAD,
                    workspace_min[1] - WS_BG_PAD - WS_TITLE_HEIGHT,
                ],
                [workspace_max[0] + WS_BG_PAD, workspace_max[1] + WS_BG_PAD],
            )
        })
}

fn minimap_point(model: &MinimapModel, canvas_x: f32, canvas_y: f32) -> Pos2 {
    Pos2::new(
        MINIMAP_PAD + (canvas_x - model.content_min[0]) * model.scale_x,
        MINIMAP_PAD + (canvas_y - model.content_min[1]) * model.scale_y,
    )
}

fn workspace_minimap_screen_rect(
    origin: Pos2,
    model: &MinimapModel,
    workspace_id: WorkspaceId,
    workspace_position: [f32; 2],
    workspace_bounds: &HashMap<WorkspaceId, ([f32; 2], [f32; 2])>,
) -> Rect {
    let (workspace_min, workspace_max) =
        workspace_minimap_bounds(workspace_id, workspace_bounds).unwrap_or_else(|| {
            (
                workspace_position,
                [
                    workspace_position[0] + WS_EMPTY_SIZE[0],
                    workspace_position[1] + WS_EMPTY_SIZE[1],
                ],
            )
        });
    Rect::from_min_max(
        origin + minimap_point(model, workspace_min[0], workspace_min[1]).to_vec2(),
        origin + minimap_point(model, workspace_max[0], workspace_max[1]).to_vec2(),
    )
}

fn panel_minimap_screen_rect(origin: Pos2, model: &MinimapModel, position: [f32; 2], size: [f32; 2]) -> Rect {
    Rect::from_min_max(
        origin + minimap_point(model, position[0], position[1]).to_vec2(),
        origin + minimap_point(model, position[0] + size[0], position[1] + size[1]).to_vec2(),
    )
}

fn paint_minimap_viewport(painter: &Painter, origin: Pos2, model: &MinimapModel) {
    let map_rect = Rect::from_min_max(
        origin + Vec2::splat(MINIMAP_PAD),
        origin + (model.outer_size - Vec2::splat(MINIMAP_PAD)),
    );
    let viewport_rect = Rect::from_min_max(
        origin + minimap_point(model, model.view_min.x, model.view_min.y).to_vec2(),
        origin + minimap_point(model, model.view_max.x, model.view_max.y).to_vec2(),
    )
    .intersect(map_rect);
    if !viewport_rect.is_positive() {
        return;
    }
    painter.rect_filled(viewport_rect, CornerRadius::same(1), theme::alpha(theme::FG(), 14));
    painter.rect_stroke(
        viewport_rect,
        CornerRadius::same(1),
        Stroke::new(1.0_f32, theme::alpha(theme::FG(), 90)),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use egui::{Pos2, Rect, Vec2};
    use horizon_core::WorkspaceId;

    use super::{
        MinimapModel, WS_EMPTY_SIZE, panel_minimap_screen_rect, vertical_label_badge_rect,
        workspace_minimap_screen_rect,
    };

    fn test_model() -> MinimapModel {
        MinimapModel {
            content_min: [100.0, 200.0],
            scale_x: 0.5,
            scale_y: 0.25,
            outer_size: Vec2::ZERO,
            view_min: Pos2::ZERO,
            view_max: Pos2::ZERO,
        }
    }

    #[test]
    fn vertical_label_badge_rect_skips_sub_two_pixel_workspaces() {
        let workspace_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(1.5, 40.0));

        let badge_rect = vertical_label_badge_rect(workspace_rect, 6.0, 24.0);

        assert_eq!(badge_rect, None);
    }

    #[test]
    fn panel_minimap_screen_rect_applies_model_scale_and_pad() {
        let rect = panel_minimap_screen_rect(Pos2::new(10.0, 20.0), &test_model(), [140.0, 240.0], [20.0, 40.0]);

        assert_eq!(rect, Rect::from_min_max(Pos2::new(36.0, 36.0), Pos2::new(46.0, 46.0)));
    }

    #[test]
    fn workspace_minimap_screen_rect_falls_back_to_empty_size() {
        let rect = workspace_minimap_screen_rect(
            Pos2::new(10.0, 20.0),
            &test_model(),
            WorkspaceId(7),
            [100.0, 200.0],
            &HashMap::new(),
        );

        assert_eq!(
            rect,
            Rect::from_min_max(
                Pos2::new(16.0, 26.0),
                Pos2::new(16.0 + WS_EMPTY_SIZE[0] * 0.5, 26.0 + WS_EMPTY_SIZE[1] * 0.25)
            )
        );
    }
}
