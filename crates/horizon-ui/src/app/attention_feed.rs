use egui::{Button, Color32, CornerRadius, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense, Vec2};
use horizon_core::{AttentionId, AttentionItem, AttentionSeverity, Board, OverlaysConfig, PanelId};
use std::time::SystemTime;

use crate::theme;

const FEED_MIN_WIDTH: f32 = 240.0;
const FEED_MARGIN: f32 = 16.0;
const FEED_ITEM_SPACING: f32 = 4.0;
const FEED_HEADER_BLOCK_HEIGHT: f32 = 30.0;
const FEED_ITEM_VIEWPORT_HEIGHT: f32 = 74.0;

#[derive(Default)]
pub struct AttentionFeedResult {
    pub focus_panel: Option<PanelId>,
    pub dismissed_ids: Vec<AttentionId>,
}

#[derive(Default)]
struct FeedItemAction {
    focus_panel: Option<PanelId>,
    dismiss_id: Option<AttentionId>,
}

/// Renders the attention feed overlay and returns any user actions.
pub fn render_attention_feed(
    ctx: &egui::Context,
    board: &Board,
    minimap_height: f32,
    overlays: &OverlaysConfig,
) -> AttentionFeedResult {
    let now = SystemTime::now();
    let items = visible_attention_items(&board.attention, now);
    if items.is_empty() {
        return AttentionFeedResult::default();
    }

    let feed_width = overlays.attention_feed_width.max(FEED_MIN_WIDTH);
    let offset_y = FEED_MARGIN + minimap_height + if minimap_height > 0.0 { 8.0 } else { 0.0 };
    let feed_target_height = overlays
        .attention_feed_height
        .max(FEED_HEADER_BLOCK_HEIGHT + FEED_ITEM_VIEWPORT_HEIGHT);
    let mut result = AttentionFeedResult::default();
    let list_max_height = snapped_feed_list_height(feed_target_height);
    let scroll_id = (
        "attention_feed_list",
        items.first().map(|item| item.id.0).unwrap_or_default(),
        items.len(),
    );

    egui::Area::new(Id::new("attention_feed"))
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-FEED_MARGIN, -offset_y))
        .order(Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(egui::Stroke::new(
                    1.0,
                    theme::blend(theme::BG_ELEVATED(), theme::FG_DIM(), 0.15),
                ))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(8, 6));

            frame.show(ui, |ui| {
                ui.set_width(feed_width);
                if feed_target_height > 0.0 {
                    ui.set_min_height(feed_target_height);
                    ui.set_max_height(feed_target_height);
                }
                render_feed_header(ui, &items);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);
                result = render_feed_list(ui, &items, feed_width, list_max_height, scroll_id);
                let remaining = if feed_target_height > 0.0 {
                    (feed_target_height - FEED_HEADER_BLOCK_HEIGHT - list_max_height).max(0.0)
                } else {
                    0.0
                };
                if remaining > 0.0 {
                    ui.add_space(remaining);
                }
            });
        });

    result
}

fn render_feed_header(ui: &mut egui::Ui, items: &[&AttentionItem]) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Attention Feed")
                .size(11.0)
                .color(theme::FG_DIM())
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let open_count = items.iter().filter(|i| i.is_open()).count();
            if open_count > 0 {
                ui.label(
                    RichText::new(format!("{open_count} open"))
                        .size(10.0)
                        .color(severity_color(AttentionSeverity::High)),
                );
            }
        });
    });
}

fn render_feed_list(
    ui: &mut egui::Ui,
    items: &[&AttentionItem],
    feed_width: f32,
    list_max_height: f32,
    scroll_id: (&'static str, u64, usize),
) -> AttentionFeedResult {
    let mut result = AttentionFeedResult::default();
    ScrollArea::vertical()
        .id_salt(scroll_id)
        .max_height(list_max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in items {
                let action = render_feed_item(ui, item, feed_width);
                if action.focus_panel.is_some() {
                    result.focus_panel = action.focus_panel;
                }
                if let Some(id) = action.dismiss_id {
                    result.dismissed_ids.push(id);
                }
                ui.add_space(FEED_ITEM_SPACING);
            }
        });
    result
}

fn render_feed_item(ui: &mut egui::Ui, item: &AttentionItem, feed_width: f32) -> FeedItemAction {
    let is_resolved = !item.is_open();
    let color = severity_color(item.severity);
    let bg_color = Color32::from_rgba_unmultiplied(
        color.r() / 8,
        color.g() / 8,
        color.b() / 8,
        if is_resolved { 20 } else { 40 },
    );
    let mut dismiss_clicked = false;
    let mut goto_clicked = false;

    let frame = egui::Frame::new()
        .fill(bg_color)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 5));

    let resp = frame
        .show(ui, |ui| {
            ui.set_width(feed_width - 16.0);
            render_feed_item_header(ui, item, is_resolved, color);
            let msg_color = if is_resolved {
                theme::alpha(theme::FG_SOFT(), 100)
            } else {
                theme::FG_SOFT()
            };
            ui.label(RichText::new(&item.summary).size(11.0).color(msg_color));

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if item.is_open() {
                        let dismiss = ui.add(
                            Button::new(RichText::new("\u{00D7}").size(11.0).color(theme::FG_DIM()))
                                .fill(theme::alpha(theme::BG_ELEVATED(), 140))
                                .corner_radius(CornerRadius::same(4))
                                .stroke(egui::Stroke::NONE),
                        );
                        dismiss_clicked = dismiss.clicked();
                    }
                    if item.is_open() && item.panel_id.is_some() {
                        let goto = ui.add(
                            egui::Button::new(RichText::new("Go to panel \u{2192}").size(9.5).color(theme::ACCENT()))
                                .fill(theme::alpha(theme::ACCENT(), 20))
                                .corner_radius(CornerRadius::same(4))
                                .stroke(egui::Stroke::NONE),
                        );
                        goto_clicked = goto.clicked();
                    }
                });
            });
        })
        .response;

    if dismiss_clicked {
        return FeedItemAction {
            dismiss_id: Some(item.id),
            ..FeedItemAction::default()
        };
    }

    if goto_clicked {
        return FeedItemAction {
            focus_panel: item.panel_id,
            ..FeedItemAction::default()
        };
    }

    // Make the whole item clickable to navigate
    if item.panel_id.is_some() {
        let interact = ui.interact(resp.rect, Id::new(("attn_item", item.id.0)), Sense::click());
        if interact.clicked() {
            return FeedItemAction {
                focus_panel: item.panel_id,
                ..FeedItemAction::default()
            };
        }
        if interact.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    FeedItemAction::default()
}

fn render_feed_item_header(ui: &mut egui::Ui, item: &AttentionItem, is_resolved: bool, color: Color32) {
    ui.horizontal(|ui| {
        // Solid filled indicator
        let indicator_rect = ui.allocate_space(Vec2::new(8.0, 8.0));
        let dot_color = if is_resolved { theme::alpha(color, 100) } else { color };
        ui.painter()
            .rect_filled(indicator_rect.1, CornerRadius::same(2), dot_color);

        let label = severity_label(item.severity);
        ui.label(
            RichText::new(label)
                .size(9.0)
                .color(theme::alpha(color, if is_resolved { 100 } else { 200 }))
                .strong(),
        );
        ui.label(
            RichText::new(&item.source)
                .size(9.0)
                .color(theme::alpha(theme::FG_DIM(), if is_resolved { 100 } else { 180 })),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let elapsed = format_elapsed(item.created_at);
            ui.label(
                RichText::new(elapsed)
                    .size(9.0)
                    .color(theme::alpha(theme::FG_DIM(), if is_resolved { 80 } else { 140 })),
            );
            if is_resolved {
                ui.label(
                    RichText::new("\u{2713}")
                        .size(9.0)
                        .color(theme::alpha(theme::PALETTE_GREEN(), 120)),
                );
            }
        });
    });
}

fn severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED(),
        AttentionSeverity::Medium => theme::PALETTE_GREEN(),
        AttentionSeverity::Low => theme::ACCENT(),
    }
}

fn severity_label(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "ATTENTION",
        AttentionSeverity::Medium => "DONE",
        AttentionSeverity::Low => "INFO",
    }
}

fn format_elapsed(time: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(time) else {
        return "now".to_string();
    };
    let secs = elapsed.as_secs();
    if secs < 5 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Estimated outer bounding rect of the attention feed overlay.  Used by
/// overlay exclusion to prevent canvas-space elements from rendering behind
/// this widget.
pub(super) fn estimated_outer_rect(
    viewport: Rect,
    minimap_height: f32,
    overlays: &OverlaysConfig,
    board: &Board,
) -> Option<Rect> {
    let now = std::time::SystemTime::now();
    if visible_attention_items(&board.attention, now).is_empty() {
        return None;
    }

    let feed_w = overlays.attention_feed_width.max(FEED_MIN_WIDTH) + 18.0;
    let feed_h = overlays
        .attention_feed_height
        .max(FEED_HEADER_BLOCK_HEIGHT + FEED_ITEM_VIEWPORT_HEIGHT)
        + 14.0;
    let offset_y = FEED_MARGIN + minimap_height + if minimap_height > 0.0 { 8.0 } else { 0.0 };

    Some(Rect::from_min_max(
        Pos2::new(
            viewport.max.x - FEED_MARGIN - feed_w,
            viewport.max.y - offset_y - feed_h,
        ),
        Pos2::new(viewport.max.x - FEED_MARGIN, viewport.max.y - offset_y),
    ))
}

fn snapped_feed_list_height(feed_target_height: f32) -> f32 {
    let available = (feed_target_height - FEED_HEADER_BLOCK_HEIGHT).max(FEED_ITEM_VIEWPORT_HEIGHT);
    let row_span = FEED_ITEM_VIEWPORT_HEIGHT + FEED_ITEM_SPACING;
    let visible_rows = ((available + FEED_ITEM_SPACING) / row_span).floor().max(1.0);
    (visible_rows * FEED_ITEM_VIEWPORT_HEIGHT) + ((visible_rows - 1.0) * FEED_ITEM_SPACING)
}

fn visible_attention_items(attention: &[AttentionItem], now: SystemTime) -> Vec<&AttentionItem> {
    let mut items: Vec<_> = attention
        .iter()
        .filter(|item| is_visible_attention_item(item, now))
        .collect();
    items.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    items.truncate(10);
    items
}

fn is_visible_attention_item(item: &AttentionItem, now: SystemTime) -> bool {
    if item.is_open() {
        return true;
    }

    item.is_resolved()
        && item
            .resolved_at
            .and_then(|resolved_at| now.duration_since(resolved_at).ok())
            .is_some_and(|elapsed| elapsed.as_secs() < 30)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use egui::{Pos2, Rect};
    use horizon_core::{
        AttentionId, AttentionItem, AttentionSeverity, AttentionState, Board, OverlaysConfig, PanelId, WorkspaceId,
    };

    use super::{estimated_outer_rect, snapped_feed_list_height, visible_attention_items};

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
        let items = visible_attention_items(&attention, now);

        let ids: Vec<_> = items.iter().map(|item| item.id.0).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn visible_attention_items_sort_newest_first_and_truncate_to_ten() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);
        let items: Vec<_> = (0_u64..12)
            .map(|id| test_attention_item(id, now - Duration::from_secs(id), AttentionState::Open, None))
            .collect();

        let visible = visible_attention_items(&items, now);

        let ids: Vec<_> = visible.iter().map(|item| item.id.0).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
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
    fn snapped_feed_list_height_preserves_complete_rows() {
        let height = snapped_feed_list_height(260.0);

        assert!((height - 230.0).abs() <= f32::EPSILON);
    }
}
