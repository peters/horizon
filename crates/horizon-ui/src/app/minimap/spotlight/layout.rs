use std::collections::HashMap;

use egui::{Pos2, Rect, Vec2};
use horizon_core::{AttentionId, AttentionSeverity, PanelId, WorkspaceId};

use super::{
    ACCESSIBLE_TARGET_SIZE, MAX_PANEL_MARKERS, PANEL_MARKER_MIN_HEIGHT, PANEL_MARKER_MIN_WIDTH, PANEL_MARKER_RADIUS,
    aggregation::{MinimapSpotlight, PanelAttentionCue},
};
use crate::app::minimap::{
    HorizonApp, MinimapPaintGeometry, MinimapScope, minimap_point, scope_includes_workspace, workspace_minimap_rect,
};

pub(in crate::app::minimap) struct MinimapCueLayout {
    pub(super) map_rect: Rect,
    pub(super) active_tabs: HashMap<WorkspaceId, ActiveTab>,
    pub(super) workspace_badges: HashMap<WorkspaceId, Rect>,
    pub(super) panel_markers: Vec<PanelMarkerLayout>,
    exclusions: Vec<Rect>,
}

#[derive(Clone, Copy)]
pub(super) struct PanelMarkerLayout {
    pub(super) workspace_id: WorkspaceId,
    pub(super) panel_id: PanelId,
    pub(super) attention_id: AttentionId,
    pub(super) severity: AttentionSeverity,
    pub(super) visual_rect: Rect,
    pub(super) hit_rect: Rect,
}

impl MinimapCueLayout {
    pub(in crate::app::minimap) fn collect_active_tabs(app: &HorizonApp, geometry: &MinimapPaintGeometry<'_>) -> Self {
        let map_rect = Rect::from_min_size(geometry.rect.min, geometry.model.outer_size);
        let mut active_tabs = HashMap::new();
        let mut exclusions = Vec::new();
        for workspace in &app.board.workspaces {
            if !scope_includes_workspace(app, geometry.scope, workspace.id) {
                continue;
            }
            let is_active = app.board.active_workspace == Some(workspace.id)
                || geometry.scope == MinimapScope::Workspace(workspace.id);
            let workspace_rect = workspace_minimap_rect(
                workspace.id,
                workspace.position,
                geometry.rect.min,
                geometry.model,
                geometry.workspace_bounds,
            );
            if is_active && let Some(tab) = active_tab_layout(workspace_rect, map_rect) {
                exclusions.push(tab.rect.expand(1.0).intersect(map_rect));
                active_tabs.insert(workspace.id, tab);
            }
        }
        Self {
            map_rect,
            active_tabs,
            workspace_badges: HashMap::new(),
            panel_markers: Vec::new(),
            exclusions,
        }
    }

    pub(in crate::app::minimap) fn exclusions(&self) -> &[Rect] {
        &self.exclusions
    }

    pub(in crate::app::minimap) fn place_attention(
        &mut self,
        app: &HorizonApp,
        geometry: &MinimapPaintGeometry<'_>,
        spotlight: &MinimapSpotlight,
        label_rects: &[Rect],
    ) {
        self.exclusions.extend_from_slice(label_rects);

        let mut workspace_ids: Vec<_> = app
            .board
            .workspaces
            .iter()
            .filter(|workspace| {
                scope_includes_workspace(app, geometry.scope, workspace.id)
                    && spotlight.workspaces.contains_key(&workspace.id)
            })
            .map(|workspace| workspace.id)
            .collect();
        workspace_ids.sort_by_key(|workspace_id| app.board.active_workspace != Some(*workspace_id));

        for workspace_id in workspace_ids {
            let Some(workspace) = app.board.workspace(workspace_id) else {
                continue;
            };
            let Some(cue) = spotlight.workspaces.get(&workspace_id) else {
                continue;
            };
            let workspace_rect = workspace_minimap_rect(
                workspace.id,
                workspace.position,
                geometry.rect.min,
                geometry.model,
                geometry.workspace_bounds,
            );
            let badge = workspace_badge_rect(workspace_rect, self.map_rect, cue.open_count, &self.exclusions);
            self.exclusions.push(accessible_hit_rect(badge, self.map_rect));
            self.workspace_badges.insert(workspace_id, badge);
        }

        for workspace in &app.board.workspaces {
            if !scope_includes_workspace(app, geometry.scope, workspace.id) {
                continue;
            }
            let Some(cue) = spotlight.workspaces.get(&workspace.id) else {
                continue;
            };
            let markers = panel_marker_layouts(
                &cue.panel_cues,
                |panel_id| panel_minimap_rect(app, geometry, workspace.id, panel_id),
                self.map_rect,
                &self.exclusions,
            );
            for (panel_cue, visual_rect) in markers {
                let hit_rect = accessible_hit_rect(visual_rect, self.map_rect);
                self.exclusions.push(hit_rect);
                self.panel_markers.push(PanelMarkerLayout {
                    workspace_id: workspace.id,
                    panel_id: panel_cue.panel_id,
                    attention_id: panel_cue.attention_id,
                    severity: panel_cue.display_severity,
                    visual_rect,
                    hit_rect,
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ActiveTab {
    pub(super) rect: Rect,
    pub(super) label: &'static str,
}

pub(super) fn active_tab_layout(workspace_rect: Rect, map_rect: Rect) -> Option<ActiveTab> {
    let (label, width): (&str, f32) = if workspace_rect.width() >= 44.0 {
        ("ACTIVE", 42.0)
    } else if workspace_rect.width() >= 28.0 {
        ("ACT", 26.0)
    } else if workspace_rect.width() >= 17.0 {
        ("A", 16.0)
    } else {
        return None;
    };
    let desired = Rect::from_min_size(
        Pos2::new(workspace_rect.max.x - width - 2.0, workspace_rect.max.y - 6.0),
        Vec2::new(width.min(workspace_rect.width()), 12.0),
    );

    Some(ActiveTab {
        rect: clamp_rect(desired, map_rect),
        label,
    })
}

fn panel_minimap_rect(
    app: &HorizonApp,
    geometry: &MinimapPaintGeometry<'_>,
    workspace_id: WorkspaceId,
    panel_id: PanelId,
) -> Option<Rect> {
    app.board
        .panel(panel_id)
        .filter(|panel| panel.workspace_id == workspace_id)
        .map(|panel| {
            Rect::from_min_max(
                geometry.rect.min
                    + minimap_point(geometry.model, panel.layout.position[0], panel.layout.position[1]).to_vec2(),
                geometry.rect.min
                    + minimap_point(
                        geometry.model,
                        panel.layout.position[0] + panel.layout.size[0],
                        panel.layout.position[1] + panel.layout.size[1],
                    )
                    .to_vec2(),
            )
        })
}

pub(super) fn panel_marker_rect(panel_rect: Rect, map_rect: Rect, reserved: &[Rect]) -> Option<Rect> {
    if panel_rect.width() < PANEL_MARKER_MIN_WIDTH || panel_rect.height() < PANEL_MARKER_MIN_HEIGHT {
        return None;
    }

    let centers = [
        Pos2::new(panel_rect.max.x - 3.0, panel_rect.min.y + 3.0),
        Pos2::new(panel_rect.max.x - 3.0, panel_rect.max.y - 3.0),
        Pos2::new(panel_rect.min.x + 3.0, panel_rect.max.y - 3.0),
        Pos2::new(panel_rect.min.x + 3.0, panel_rect.min.y + 3.0),
    ];
    centers.into_iter().find_map(|center| {
        let marker = clamp_rect(
            Rect::from_center_size(center, Vec2::splat(PANEL_MARKER_RADIUS * 2.0)),
            map_rect,
        );
        let hit_rect = accessible_hit_rect(marker, map_rect);
        reserved
            .iter()
            .all(|reserved_rect| !reserved_rect.intersects(hit_rect))
            .then_some(marker)
    })
}

pub(super) fn panel_marker_layouts<'a>(
    cues: &'a [PanelAttentionCue],
    mut panel_rect: impl FnMut(PanelId) -> Option<Rect>,
    map_rect: Rect,
    reserved: &[Rect],
) -> Vec<(&'a PanelAttentionCue, Rect)> {
    let mut occupied = reserved.to_vec();
    let mut markers = Vec::with_capacity(MAX_PANEL_MARKERS);
    for cue in cues {
        if markers.len() == MAX_PANEL_MARKERS {
            break;
        }
        let Some(marker_rect) = panel_rect(cue.panel_id).and_then(|rect| panel_marker_rect(rect, map_rect, &occupied))
        else {
            continue;
        };
        occupied.push(accessible_hit_rect(marker_rect, map_rect));
        markers.push((cue, marker_rect));
    }
    markers
}

pub(super) fn workspace_badge_rect(workspace_rect: Rect, map_rect: Rect, open_count: usize, reserved: &[Rect]) -> Rect {
    let width = if open_count == 0 {
        18.0
    } else if open_count > 9 {
        30.0
    } else {
        24.0
    };
    let size = Vec2::new(width, 15.0);
    let positions = [
        Pos2::new(workspace_rect.max.x - width + 4.0, workspace_rect.min.y - 7.0),
        Pos2::new(workspace_rect.min.x - 4.0, workspace_rect.min.y - 7.0),
        Pos2::new(workspace_rect.max.x + 3.0, workspace_rect.center().y - size.y * 0.5),
        Pos2::new(
            workspace_rect.min.x - width - 3.0,
            workspace_rect.center().y - size.y * 0.5,
        ),
        Pos2::new(workspace_rect.max.x - width + 4.0, workspace_rect.max.y - 8.0),
        Pos2::new(workspace_rect.min.x - 4.0, workspace_rect.max.y - 8.0),
        Pos2::new(workspace_rect.center().x - width * 0.5, workspace_rect.min.y - 18.0),
        Pos2::new(workspace_rect.center().x - width * 0.5, workspace_rect.max.y + 3.0),
    ];
    let candidates = positions.map(|position| clamp_rect(Rect::from_min_size(position, size), map_rect));
    candidates
        .iter()
        .copied()
        .find(|candidate| {
            let hit_rect = accessible_hit_rect(*candidate, map_rect);
            reserved.iter().all(|occupied| !occupied.intersects(hit_rect))
        })
        .unwrap_or_else(|| {
            candidates
                .into_iter()
                .min_by(|left, right| {
                    overlap_area(accessible_hit_rect(*left, map_rect), reserved)
                        .total_cmp(&overlap_area(accessible_hit_rect(*right, map_rect), reserved))
                })
                .unwrap_or_else(|| Rect::from_min_size(map_rect.min, size))
        })
}

pub(super) fn accessible_hit_rect(visual_rect: Rect, map_rect: Rect) -> Rect {
    clamp_rect(
        Rect::from_center_size(
            visual_rect.center(),
            Vec2::new(
                visual_rect.width().max(ACCESSIBLE_TARGET_SIZE),
                visual_rect.height().max(ACCESSIBLE_TARGET_SIZE),
            ),
        ),
        map_rect,
    )
}

fn overlap_area(rect: Rect, occupied: &[Rect]) -> f32 {
    occupied
        .iter()
        .map(|occupied_rect| {
            let overlap = rect.intersect(*occupied_rect);
            overlap.width().max(0.0) * overlap.height().max(0.0)
        })
        .sum()
}

fn clamp_rect(rect: Rect, bounds: Rect) -> Rect {
    let max_x = (bounds.max.x - rect.width()).max(bounds.min.x);
    let max_y = (bounds.max.y - rect.height()).max(bounds.min.y);
    let min = Pos2::new(
        rect.min.x.clamp(bounds.min.x, max_x),
        rect.min.y.clamp(bounds.min.y, max_y),
    );
    Rect::from_min_size(min, rect.size())
}
