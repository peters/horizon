use std::time::{Duration, SystemTime};

use egui::{Pos2, Rect, Vec2};
use horizon_core::{AttentionId, AttentionItem, AttentionSeverity, AttentionState, PanelId, WorkspaceId};

use super::super::{MinimapScope, scope_includes_workspace_state};
use super::{
    ACCESSIBLE_TARGET_SIZE, MinimapNavigationTarget,
    aggregation::{aggregate_attention, project_attention},
    layout::{accessible_hit_rect, active_tab_layout, panel_marker_layouts, panel_marker_rect, workspace_badge_rect},
    workspace_badge_text,
};

fn attention_item(
    id: u64,
    workspace_id: WorkspaceId,
    panel_id: Option<PanelId>,
    severity: AttentionSeverity,
    state: AttentionState,
    created_at: SystemTime,
    resolved_at: Option<SystemTime>,
) -> AttentionItem {
    let mut item = AttentionItem::new(
        AttentionId(id),
        workspace_id,
        panel_id,
        "test",
        format!("item-{id}"),
        severity,
    );
    item.state = state;
    item.created_at = created_at;
    item.resolved_at = resolved_at;
    item
}

#[test]
fn aggregation_matches_attention_feed_visibility() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let workspace_id = WorkspaceId(1);
    let attention = [
        attention_item(
            1,
            workspace_id,
            None,
            AttentionSeverity::Low,
            AttentionState::Open,
            now - Duration::from_secs(90),
            None,
        ),
        attention_item(
            2,
            workspace_id,
            None,
            AttentionSeverity::High,
            AttentionState::Resolved,
            now - Duration::from_secs(10),
            Some(now - Duration::from_secs(29)),
        ),
        attention_item(
            3,
            WorkspaceId(2),
            None,
            AttentionSeverity::High,
            AttentionState::Resolved,
            now - Duration::from_secs(10),
            Some(now - Duration::from_secs(30)),
        ),
        attention_item(
            4,
            WorkspaceId(3),
            None,
            AttentionSeverity::High,
            AttentionState::Dismissed,
            now,
            Some(now),
        ),
    ];

    let spotlight = aggregate_attention(&attention, now, |_| true);

    assert_eq!(spotlight.workspaces.len(), 1);
    assert!(spotlight.workspaces.contains_key(&workspace_id));
}

#[test]
fn open_attention_takes_precedence_over_recently_resolved_attention() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(200);
    let workspace_id = WorkspaceId(1);
    let attention = [
        attention_item(
            1,
            workspace_id,
            Some(PanelId(1)),
            AttentionSeverity::Low,
            AttentionState::Open,
            now - Duration::from_secs(20),
            None,
        ),
        attention_item(
            2,
            workspace_id,
            Some(PanelId(2)),
            AttentionSeverity::High,
            AttentionState::Resolved,
            now - Duration::from_secs(5),
            Some(now - Duration::from_secs(2)),
        ),
    ];

    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");

    assert_eq!(cue.display_severity, AttentionSeverity::Low);
    assert_eq!(cue.open_count, 1);
    assert_eq!(cue.panel_cues.len(), 1);
    assert_eq!(cue.panel_cues[0].panel_id, PanelId(1));
}

#[test]
fn aggregation_uses_highest_severity_and_counts_all_open_items() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(300);
    let workspace_id = WorkspaceId(1);
    let mut attention = Vec::new();
    for id in 1_u64..=10 {
        attention.push(attention_item(
            id,
            workspace_id,
            Some(PanelId(id)),
            if id == 10 {
                AttentionSeverity::High
            } else {
                AttentionSeverity::Low
            },
            AttentionState::Open,
            now - Duration::from_secs(10 - id),
            None,
        ));
    }

    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");

    assert_eq!(cue.display_severity, AttentionSeverity::High);
    assert_eq!(cue.open_count, 10);
    assert_eq!(workspace_badge_text(cue), "!9+");
    assert_eq!(cue.panel_cues.len(), 10);
}

#[test]
fn workspace_only_attention_targets_workspace_without_panel_marker() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(400);
    let workspace_id = WorkspaceId(7);
    let attention = [attention_item(
        1,
        workspace_id,
        None,
        AttentionSeverity::High,
        AttentionState::Open,
        now,
        None,
    )];

    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");

    assert_eq!(cue.target, MinimapNavigationTarget::Workspace(workspace_id));
    assert!(cue.panel_cues.is_empty());
}

#[test]
fn click_target_prefers_severity_before_recency_and_recency_breaks_ties() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(500);
    let workspace_id = WorkspaceId(1);
    let attention = [
        attention_item(
            1,
            workspace_id,
            Some(PanelId(1)),
            AttentionSeverity::High,
            AttentionState::Open,
            now - Duration::from_secs(20),
            None,
        ),
        attention_item(
            2,
            workspace_id,
            Some(PanelId(2)),
            AttentionSeverity::Medium,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            3,
            workspace_id,
            Some(PanelId(3)),
            AttentionSeverity::High,
            AttentionState::Open,
            now - Duration::from_secs(10),
            None,
        ),
    ];

    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");

    assert_eq!(
        cue.target,
        MinimapNavigationTarget::Panel {
            workspace_id,
            panel_id: PanelId(3),
        }
    );
}

#[test]
fn recent_resolved_attention_becomes_completed_cue() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(600);
    let workspace_id = WorkspaceId(1);
    let attention = [attention_item(
        1,
        workspace_id,
        Some(PanelId(4)),
        AttentionSeverity::High,
        AttentionState::Resolved,
        now - Duration::from_secs(5),
        Some(now - Duration::from_secs(1)),
    )];

    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");

    assert_eq!(cue.display_severity, AttentionSeverity::Medium);
    assert_eq!(cue.open_count, 0);
    assert_eq!(workspace_badge_text(cue), "✓");
}

#[test]
fn detached_scope_projects_attention_through_real_minimap_scope_rules() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(700);
    let detached_workspace = WorkspaceId(2);
    let attention = [
        attention_item(
            1,
            WorkspaceId(1),
            None,
            AttentionSeverity::High,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            2,
            detached_workspace,
            None,
            AttentionSeverity::Low,
            AttentionState::Open,
            now,
            None,
        ),
    ];

    let attached_spotlight = aggregate_attention(&attention, now, |workspace_id| {
        scope_includes_workspace_state(MinimapScope::Attached, workspace_id, workspace_id == detached_workspace)
    });
    let detached_spotlight = aggregate_attention(&attention, now, |workspace_id| {
        scope_includes_workspace_state(
            MinimapScope::Workspace(detached_workspace),
            workspace_id,
            workspace_id == detached_workspace,
        )
    });

    assert_eq!(attached_spotlight.workspaces.len(), 1);
    assert!(attached_spotlight.workspaces.contains_key(&WorkspaceId(1)));
    assert!(!attached_spotlight.workspaces.contains_key(&detached_workspace));
    assert_eq!(detached_spotlight.workspaces.len(), 1);
    assert!(detached_spotlight.workspaces.contains_key(&detached_workspace));
}

#[test]
fn disabled_attention_feature_produces_no_minimap_cues() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(725);
    let attention = [attention_item(
        1,
        WorkspaceId(1),
        None,
        AttentionSeverity::High,
        AttentionState::Open,
        now,
        None,
    )];

    let spotlight = project_attention(&attention, now, false, |_| true);

    assert!(spotlight.workspaces.is_empty());
}

#[test]
fn marker_requires_large_geometry_and_hit_target_remains_accessible() {
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 120.0));
    let tiny_panel = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(17.0, 15.0));
    let large_panel = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::new(40.0, 30.0));

    assert!(panel_marker_rect(tiny_panel, map_rect, &[]).is_none());
    let marker = panel_marker_rect(large_panel, map_rect, &[]).expect("marker");
    let hit = accessible_hit_rect(marker, map_rect);
    assert!(hit.width() >= ACCESSIBLE_TARGET_SIZE);
    assert!(hit.height() >= ACCESSIBLE_TARGET_SIZE);
}

#[test]
fn marker_layout_skips_tiny_panels_before_limiting_to_two() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(750);
    let workspace_id = WorkspaceId(1);
    let attention = [
        attention_item(
            1,
            workspace_id,
            Some(PanelId(1)),
            AttentionSeverity::High,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            2,
            workspace_id,
            Some(PanelId(2)),
            AttentionSeverity::Medium,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            3,
            workspace_id,
            Some(PanelId(3)),
            AttentionSeverity::Low,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            4,
            workspace_id,
            Some(PanelId(4)),
            AttentionSeverity::Low,
            AttentionState::Open,
            now - Duration::from_secs(1),
            None,
        ),
    ];
    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 120.0));

    let markers = panel_marker_layouts(
        &cue.panel_cues,
        |panel_id| {
            Some(if panel_id == PanelId(1) {
                Rect::from_min_size(Pos2::new(5.0, 5.0), Vec2::new(10.0, 10.0))
            } else {
                let x = match panel_id.0 {
                    2 => 20.0,
                    3 => 60.0,
                    _ => 90.0,
                };
                Rect::from_min_size(Pos2::new(x, 20.0), Vec2::new(20.0, 20.0))
            })
        },
        map_rect,
        &[],
    );

    let panel_ids: Vec<_> = markers.iter().map(|(cue, _)| cue.panel_id).collect();
    assert_eq!(panel_ids, vec![PanelId(2), PanelId(3)]);
}

#[test]
fn marker_layout_avoids_badge_and_other_accessible_hit_targets() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(760);
    let workspace_id = WorkspaceId(1);
    let attention = [
        attention_item(
            1,
            workspace_id,
            Some(PanelId(1)),
            AttentionSeverity::High,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            2,
            workspace_id,
            Some(PanelId(2)),
            AttentionSeverity::Low,
            AttentionState::Open,
            now,
            None,
        ),
        attention_item(
            3,
            workspace_id,
            Some(PanelId(3)),
            AttentionSeverity::Low,
            AttentionState::Open,
            now,
            None,
        ),
    ];
    let spotlight = aggregate_attention(&attention, now, |_| true);
    let cue = spotlight.workspaces.get(&workspace_id).expect("workspace cue");
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(160.0, 120.0));
    let panel_rect = Rect::from_min_size(Pos2::new(50.0, 20.0), Vec2::new(70.0, 70.0));
    let top_right_marker = panel_marker_rect(panel_rect, map_rect, &[]).expect("top-right marker");
    let badge_hit = accessible_hit_rect(top_right_marker, map_rect);

    let markers = panel_marker_layouts(&cue.panel_cues, |_| Some(panel_rect), map_rect, &[badge_hit]);

    assert_eq!(markers.len(), 2);
    let marker_hits: Vec<_> = markers
        .iter()
        .map(|(_, marker)| accessible_hit_rect(*marker, map_rect))
        .collect();
    assert!(marker_hits.iter().all(|hit| !hit.intersects(badge_hit)));
    assert!(!marker_hits[0].intersects(marker_hits[1]));
}

#[test]
fn active_tab_adapts_and_clamps_to_map_bounds() {
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 120.0));
    let full = active_tab_layout(
        Rect::from_min_size(Pos2::new(92.0, 104.0), Vec2::new(48.0, 20.0)),
        map_rect,
    )
    .expect("full tab");
    let compact = active_tab_layout(
        Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(20.0, 20.0)),
        map_rect,
    )
    .expect("compact tab");

    assert_eq!(full.label, "ACTIVE");
    assert_eq!(compact.label, "A");
    assert!(full.rect.min.x >= map_rect.min.x && full.rect.max.x <= map_rect.max.x);
    assert!(full.rect.min.y >= map_rect.min.y && full.rect.max.y <= map_rect.max.y);
    assert!(active_tab_layout(Rect::from_min_size(Pos2::ZERO, Vec2::new(12.0, 20.0)), map_rect).is_none());
}

#[test]
fn workspace_badge_clamps_at_top_right_edge() {
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 120.0));
    let workspace_rect = Rect::from_min_size(Pos2::new(110.0, -4.0), Vec2::new(20.0, 20.0));

    let badge = workspace_badge_rect(workspace_rect, map_rect, 12, &[]);

    assert!(badge.min.x >= map_rect.min.x && badge.max.x <= map_rect.max.x);
    assert!(badge.min.y >= map_rect.min.y && badge.max.y <= map_rect.max.y);
}

#[test]
fn compressed_workspace_badges_reserve_global_hit_targets_and_active_tab() {
    let map_rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 120.0));
    let workspace_rects = [
        Rect::from_min_size(Pos2::new(8.0, 8.0), Vec2::new(30.0, 18.0)),
        Rect::from_min_size(Pos2::new(45.0, 8.0), Vec2::new(30.0, 18.0)),
        Rect::from_min_size(Pos2::new(8.0, 45.0), Vec2::new(30.0, 18.0)),
        Rect::from_min_size(Pos2::new(45.0, 45.0), Vec2::new(30.0, 18.0)),
    ];
    let active_tab = active_tab_layout(workspace_rects[0], map_rect).expect("active tab");
    let tab_exclusion = active_tab.rect.expand(1.0).intersect(map_rect);
    let mut occupied = vec![tab_exclusion];
    let mut badge_hits = Vec::new();

    for workspace_rect in workspace_rects {
        let badge = workspace_badge_rect(workspace_rect, map_rect, 1, &occupied);
        let hit_rect = accessible_hit_rect(badge, map_rect);
        assert!(occupied.iter().all(|reserved| !reserved.intersects(hit_rect)));
        occupied.push(hit_rect);
        badge_hits.push(hit_rect);
    }

    assert_eq!(badge_hits.len(), 4);
    assert!(badge_hits.iter().all(|hit| !hit.intersects(tab_exclusion)));
}
