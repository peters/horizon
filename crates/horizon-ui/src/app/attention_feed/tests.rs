use std::time::{Duration, SystemTime};

use egui::{Pos2, Rect};
use horizon_core::{
    AttentionId, AttentionItem, AttentionSeverity, AttentionState, Board, OverlaysConfig, PanelId, WorkspaceId,
};

use super::{attention_feed_layout, estimated_outer_rect, snapped_feed_list_height, visible_attention_items};

fn test_attention_item(
    id: u64,
    created_at: SystemTime,
    state: AttentionState,
    resolved_at: Option<SystemTime>,
) -> AttentionItem {
    let mut item = AttentionItem::new(
        AttentionId(id),
        WorkspaceId(1),
        Some(PanelId(id)),
        "agent",
        format!("item-{id}"),
        AttentionSeverity::High,
    );
    item.created_at = created_at;
    item.state = state;
    item.resolved_at = resolved_at;
    item
}

#[test]
fn visible_attention_items_include_open_and_recently_resolved_items() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    let open = test_attention_item(1, now - Duration::from_secs(5), AttentionState::Open, None);
    let recent_resolved = test_attention_item(
        2,
        now - Duration::from_secs(10),
        AttentionState::Resolved,
        Some(now - Duration::from_secs(29)),
    );
    let stale_resolved = test_attention_item(
        3,
        now - Duration::from_secs(20),
        AttentionState::Resolved,
        Some(now - Duration::from_secs(31)),
    );
    let dismissed = test_attention_item(
        4,
        now - Duration::from_secs(30),
        AttentionState::Dismissed,
        Some(now - Duration::from_secs(5)),
    );

    let attention = [open, recent_resolved, stale_resolved, dismissed];
    let visible = visible_attention_items(&attention, now);

    let ids: Vec<_> = visible.items.iter().map(|item| item.id.0).collect();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(visible.open_count, 1);
}

#[test]
fn visible_attention_items_never_hide_open_items_behind_resolved_history() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);
    let mut items = vec![test_attention_item(
        100,
        now - Duration::from_secs(20),
        AttentionState::Open,
        None,
    )];
    items.extend((0_u64..12).map(|id| {
        test_attention_item(
            id,
            now - Duration::from_secs(id),
            AttentionState::Resolved,
            Some(now - Duration::from_secs(1)),
        )
    }));

    let visible = visible_attention_items(&items, now);

    assert_eq!(visible.open_count, 1);
    assert_eq!(visible.items.first().map(|item| item.id), Some(AttentionId(100)));
    assert_eq!(visible.items.len(), 11);
}

#[test]
fn visible_attention_items_keep_every_open_item() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(3000);
    let items: Vec<_> = (0_u64..12)
        .map(|id| test_attention_item(id, now - Duration::from_secs(id), AttentionState::Open, None))
        .collect();

    let visible = visible_attention_items(&items, now);

    assert_eq!(visible.open_count, 12);
    assert_eq!(visible.items.len(), 12);
}

#[test]
fn visible_attention_items_break_equal_open_timestamps_by_newest_id() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(4000);
    let items = [
        test_attention_item(1, now, AttentionState::Open, None),
        test_attention_item(3, now, AttentionState::Open, None),
        test_attention_item(2, now, AttentionState::Open, None),
    ];

    let visible = visible_attention_items(&items, now);
    let ids: Vec<_> = visible.items.iter().map(|item| item.id.0).collect();

    assert_eq!(ids, vec![3, 2, 1]);
}

#[test]
fn visible_attention_items_order_history_by_resolution_time() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(5000);
    let items = [
        test_attention_item(
            1,
            now - Duration::from_secs(200),
            AttentionState::Resolved,
            Some(now - Duration::from_secs(1)),
        ),
        test_attention_item(
            2,
            now - Duration::from_secs(10),
            AttentionState::Resolved,
            Some(now - Duration::from_secs(5)),
        ),
    ];

    let visible = visible_attention_items(&items, now);
    let ids: Vec<_> = visible.items.iter().map(|item| item.id.0).collect();

    assert_eq!(ids, vec![1, 2]);
}

#[test]
fn estimated_outer_rect_hides_when_only_stale_items_remain() {
    let now = SystemTime::now();
    let mut board = Board::new();
    board.attention.push(test_attention_item(
        1,
        now - Duration::from_secs(60),
        AttentionState::Resolved,
        Some(now - Duration::from_secs(45)),
    ));

    let rect = estimated_outer_rect(
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0)),
        0.0,
        &OverlaysConfig::default(),
        &board,
    );

    assert_eq!(rect, None);
}

#[test]
fn estimated_outer_rect_uses_feed_minimum_size() {
    let mut board = Board::new();
    board.attention.push(AttentionItem::new(
        AttentionId(1),
        WorkspaceId(1),
        Some(PanelId(1)),
        "agent",
        "Ready for input",
        AttentionSeverity::High,
    ));

    let rect = estimated_outer_rect(
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1200.0, 800.0)),
        180.0,
        &OverlaysConfig {
            attention_feed_width: 120.0,
            attention_feed_height: 100.0,
            minimap_height: 180.0,
            minimap_width: 320.0,
        },
        &board,
    )
    .expect("attention feed rect");

    assert_eq!(rect.max, Pos2::new(1184.0, 596.0));
    assert!((rect.width() - 258.0).abs() <= f32::EPSILON);
    assert!((rect.height() - 118.0).abs() <= f32::EPSILON);
}

#[test]
fn default_feed_clamps_between_toolbar_and_minimap_at_1024_by_768() {
    let canvas = Rect::from_min_max(Pos2::new(184.32, 46.0), Pos2::new(1024.0, 768.0));

    let layout = attention_feed_layout(canvas, 192.0, &OverlaysConfig::default()).expect("feed layout");

    assert!(layout.outer_rect.min.y >= canvas.min.y + 16.0);
    assert!((layout.outer_rect.max.y - 552.0).abs() <= f32::EPSILON);
    assert!(layout.outer_rect.min.x >= canvas.min.x + 16.0);
    assert!(layout.outer_rect.height() < 614.0);
}

#[test]
fn configured_feed_width_clamps_inside_available_canvas() {
    let canvas = Rect::from_min_max(Pos2::new(168.0, 46.0), Pos2::new(800.0, 600.0));
    let overlays = OverlaysConfig {
        attention_feed_width: 800.0,
        ..OverlaysConfig::default()
    };

    let layout = attention_feed_layout(canvas, 0.0, &overlays).expect("feed layout");

    assert!((layout.outer_rect.min.x - (canvas.min.x + 16.0)).abs() <= f32::EPSILON);
    assert!((layout.outer_rect.max.x - (canvas.max.x - 16.0)).abs() <= f32::EPSILON);
}

#[test]
fn snapped_feed_list_height_preserves_complete_rows() {
    let height = snapped_feed_list_height(260.0);

    assert!((height - 230.0).abs() <= f32::EPSILON);
}
