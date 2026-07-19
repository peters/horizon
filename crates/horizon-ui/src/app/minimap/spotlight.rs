use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType,
};
use horizon_core::{AttentionId, AttentionSeverity, PanelId, WorkspaceId};

use crate::theme;

use super::{
    HorizonApp, MinimapPaintGeometry, MinimapScope, minimap_point, scope_includes_workspace, workspace_minimap_rect,
};

const MAX_PANEL_MARKERS: usize = 2;
const PANEL_MARKER_MIN_WIDTH: f32 = 18.0;
const PANEL_MARKER_MIN_HEIGHT: f32 = 16.0;
const PANEL_MARKER_RADIUS: f32 = 6.0;
const ACCESSIBLE_TARGET_SIZE: f32 = 24.0;

mod aggregation;
mod layout;

pub(super) use aggregation::MinimapSpotlight;
use aggregation::WorkspaceAttentionCue;
pub(super) use layout::MinimapCueLayout;
use layout::{ActiveTab, accessible_hit_rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MinimapNavigationTarget {
    Panel {
        workspace_id: WorkspaceId,
        panel_id: PanelId,
    },
    Workspace(WorkspaceId),
}

#[derive(Default)]
pub(super) struct MinimapPaintResult {
    pub(super) navigation: Option<MinimapNavigationTarget>,
}

pub(super) fn paint_minimap_scene(app: &HorizonApp, painter: &Painter, geometry: &MinimapPaintGeometry<'_>) {
    let origin = geometry.rect.min;
    let model = geometry.model;
    let workspace_bounds = geometry.workspace_bounds;
    let scope = geometry.scope;

    for workspace in &app.board.workspaces {
        if !scope_includes_workspace(app, scope, workspace.id) {
            continue;
        }

        let workspace_rect = workspace_minimap_rect(workspace.id, workspace.position, origin, model, workspace_bounds);
        let (red, green, blue) = workspace.accent();
        let workspace_color = Color32::from_rgb(red, green, blue);
        let is_active =
            app.board.active_workspace == Some(workspace.id) || scope == MinimapScope::Workspace(workspace.id);

        paint_workspace(painter, workspace_rect, workspace_color, is_active);
    }

    for panel in &app.board.panels {
        if !scope_includes_workspace(app, scope, panel.workspace_id) {
            continue;
        }

        let panel_rect = Rect::from_min_max(
            origin + minimap_point(model, panel.layout.position[0], panel.layout.position[1]).to_vec2(),
            origin
                + minimap_point(
                    model,
                    panel.layout.position[0] + panel.layout.size[0],
                    panel.layout.position[1] + panel.layout.size[1],
                )
                .to_vec2(),
        );
        let workspace_color = app
            .board
            .workspace(panel.workspace_id)
            .map_or(theme::ACCENT(), |workspace| {
                let (red, green, blue) = workspace.accent();
                Color32::from_rgb(red, green, blue)
            });
        painter.rect_filled(
            panel_rect,
            CornerRadius::same(1),
            theme::alpha(
                workspace_color,
                if app.board.focused == Some(panel.id) { 120 } else { 70 },
            ),
        );
    }
}

pub(super) fn paint_minimap_cues(
    app: &HorizonApp,
    ui: &mut Ui,
    painter: &Painter,
    spotlight: &MinimapSpotlight,
    layout: &MinimapCueLayout,
) -> MinimapPaintResult {
    paint_active_tabs(app, painter, layout);
    let panel_navigation = paint_panel_attention_cues(app, ui, painter, layout);
    let workspace_navigation = paint_workspace_attention_cues(app, ui, painter, spotlight, layout);

    MinimapPaintResult {
        navigation: panel_navigation.or(workspace_navigation),
    }
}

fn paint_active_tabs(app: &HorizonApp, painter: &Painter, layout: &MinimapCueLayout) {
    for workspace in &app.board.workspaces {
        if let Some(tab) = layout.active_tabs.get(&workspace.id) {
            paint_active_tab(painter, tab);
        }
    }
}

fn paint_panel_attention_cues(
    app: &HorizonApp,
    ui: &mut Ui,
    painter: &Painter,
    layout: &MinimapCueLayout,
) -> Option<MinimapNavigationTarget> {
    let mut navigation = None;
    for marker in &layout.panel_markers {
        paint_panel_marker(painter, marker.visual_rect, marker.severity);
        let response = minimap_button_response(
            ui,
            marker.hit_rect,
            Id::new(("minimap_panel_attention", marker.workspace_id.0, marker.panel_id.0)),
            "Focus and center attention panel",
        );
        let clicked = response.clicked();
        if response.hovered() {
            let summary = attention_summary(app, marker.attention_id).unwrap_or("Attention item");
            let _ = response.on_hover_text(format!(
                "{}: {summary}. Focus and center this panel.",
                severity_label(marker.severity),
            ));
        }
        if clicked && navigation.is_none() {
            navigation = Some(MinimapNavigationTarget::Panel {
                workspace_id: marker.workspace_id,
                panel_id: marker.panel_id,
            });
        }
    }
    navigation
}

fn paint_workspace_attention_cues(
    app: &HorizonApp,
    ui: &mut Ui,
    painter: &Painter,
    spotlight: &MinimapSpotlight,
    layout: &MinimapCueLayout,
) -> Option<MinimapNavigationTarget> {
    let mut navigation = None;
    for workspace in &app.board.workspaces {
        let Some(cue) = spotlight.workspaces.get(&workspace.id) else {
            continue;
        };
        let Some(&badge_rect) = layout.workspace_badges.get(&workspace.id) else {
            continue;
        };
        let badge_text = workspace_badge_text(cue);
        paint_workspace_badge(painter, badge_rect, &badge_text, cue.display_severity);

        let hit_rect = accessible_hit_rect(badge_rect, layout.map_rect);
        let response = minimap_button_response(
            ui,
            hit_rect,
            Id::new(("minimap_workspace_attention", workspace.id.0)),
            "Focus highest-priority workspace attention",
        );
        let clicked = response.clicked();
        if response.hovered() {
            let summary = attention_summary(app, cue.attention_id).unwrap_or("Attention item");
            let tooltip = if cue.open_count == 0 {
                format!(
                    "Recently completed attention in {}: {summary}. Focus its target.",
                    workspace.name
                )
            } else {
                format!(
                    "{} open attention item{} in {}: {summary}. Focus the highest-priority target.",
                    cue.open_count,
                    if cue.open_count == 1 { "" } else { "s" },
                    workspace.name,
                )
            };
            let _ = response.on_hover_text(tooltip);
        }
        if clicked && navigation.is_none() {
            navigation = Some(cue.target);
        }
    }

    navigation
}

fn paint_workspace(painter: &Painter, rect: Rect, workspace_color: Color32, is_active: bool) {
    painter.rect_filled(
        rect,
        CornerRadius::same(2),
        theme::alpha(workspace_color, if is_active { 60 } else { 22 }),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(0.8_f32, theme::alpha(workspace_color, if is_active { 210 } else { 80 })),
        StrokeKind::Outside,
    );

    if is_active {
        painter.rect_stroke(
            rect.expand(3.5),
            CornerRadius::same(4),
            Stroke::new(2.0_f32, theme::alpha(theme::ACCENT(), 180)),
            StrokeKind::Outside,
        );
        painter.rect_stroke(
            rect.expand(1.3),
            CornerRadius::same(3),
            Stroke::new(1.2_f32, theme::alpha(theme::FG(), 220)),
            StrokeKind::Outside,
        );
    }
}

fn paint_active_tab(painter: &Painter, tab: &ActiveTab) {
    painter.rect_filled(tab.rect, CornerRadius::same(5), theme::ACCENT());
    painter.text(
        tab.rect.center(),
        Align2::CENTER_CENTER,
        tab.label,
        FontId::proportional(7.5),
        theme::BG(),
    );
}

fn paint_panel_marker(painter: &Painter, rect: Rect, severity: AttentionSeverity) {
    let color = severity_color(severity);
    painter.circle_filled(
        rect.center(),
        PANEL_MARKER_RADIUS,
        theme::alpha(theme::BG_ELEVATED(), 245),
    );
    painter.circle_stroke(rect.center(), PANEL_MARKER_RADIUS, Stroke::new(1.5_f32, color));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        severity_icon(severity),
        FontId::proportional(8.5),
        color,
    );
}

fn workspace_badge_text(cue: &WorkspaceAttentionCue) -> String {
    if cue.open_count == 0 {
        return "✓".to_string();
    }

    let count = if cue.open_count > 9 {
        "9+".to_string()
    } else {
        cue.open_count.to_string()
    };
    format!("{}{count}", severity_icon(cue.display_severity))
}

fn paint_workspace_badge(painter: &Painter, rect: Rect, text: &str, severity: AttentionSeverity) {
    let color = severity_color(severity);
    painter.rect_filled(rect, CornerRadius::same(7), theme::alpha(theme::BG_ELEVATED(), 245));
    painter.rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(1.4_f32, color),
        StrokeKind::Outside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(8.0),
        color,
    );
}

fn minimap_button_response(ui: &mut Ui, rect: Rect, id: Id, accessibility_label: &'static str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, accessibility_label));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn attention_summary(app: &HorizonApp, attention_id: AttentionId) -> Option<&str> {
    app.board
        .attention
        .iter()
        .find(|item| item.id == attention_id)
        .map(|item| item.summary.as_str())
}

fn severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED(),
        AttentionSeverity::Medium => theme::PALETTE_GREEN(),
        AttentionSeverity::Low => theme::ACCENT(),
    }
}

fn severity_icon(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "!",
        AttentionSeverity::Medium => "✓",
        AttentionSeverity::Low => "i",
    }
}

fn severity_label(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "Needs attention",
        AttentionSeverity::Medium => "Completed",
        AttentionSeverity::Low => "Information",
    }
}

#[cfg(test)]
mod tests;
