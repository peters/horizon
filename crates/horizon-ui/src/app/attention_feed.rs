use egui::{Button, Color32, CornerRadius, Id, Order, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{
    AttentionId, AttentionItem, AttentionSeverity, Board, OverlaysConfig, PanelId, RESOLVED_ATTENTION_RETENTION,
};
use std::time::SystemTime;

use crate::theme;

pub(super) const FEED_MIN_WIDTH: f32 = 240.0;
pub(super) const FEED_MIN_HEIGHT: f32 = FEED_HEADER_BLOCK_HEIGHT + FEED_ITEM_VIEWPORT_HEIGHT;
const FEED_MARGIN: f32 = 16.0;
const FEED_ITEM_SPACING: f32 = 4.0;
const FEED_HEADER_BLOCK_HEIGHT: f32 = 30.0;
const FEED_ITEM_VIEWPORT_HEIGHT: f32 = 74.0;
const FEED_FRAME_HORIZONTAL_INSET: f32 = 18.0;
const FEED_FRAME_VERTICAL_INSET: f32 = 14.0;
const FEED_MINIMAP_GAP: f32 = 8.0;
const MAX_RECENT_RESOLVED_ITEMS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
struct AttentionFeedLayout {
    content_size: Vec2,
    outer_rect: Rect,
}

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
    available_rect: Rect,
    minimap_height: f32,
    overlays: &OverlaysConfig,
) -> AttentionFeedResult {
    let now = SystemTime::now();
    let visible = visible_attention_items(&board.attention, now);
    let items = visible.items;
    if items.is_empty() {
        return AttentionFeedResult::default();
    }

    let Some(layout) = attention_feed_layout(available_rect, minimap_height, overlays) else {
        return AttentionFeedResult::default();
    };
    let feed_width = layout.content_size.x;
    let feed_target_height = layout.content_size.y;
    let mut result = AttentionFeedResult::default();
    let list_max_height = snapped_feed_list_height(feed_target_height);
    let scroll_id = (
        "attention_feed_list",
        items.first().map(|item| item.id.0).unwrap_or_default(),
        items.len(),
    );

    egui::Area::new(Id::new("attention_feed"))
        .fixed_pos(layout.outer_rect.min)
        .movable(false)
        .constrain(false)
        .order(Order::Tooltip)
        .interactable(true)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(theme::BG_ELEVATED())
                .stroke(egui::Stroke::new(
                    1.0_f32,
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
                render_feed_header(ui, visible.open_count);
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

fn render_feed_header(ui: &mut egui::Ui, open_count: usize) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Attention Feed")
                .size(11.0)
                .color(theme::FG_DIM())
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
            for (index, item) in items.iter().enumerate() {
                let action = render_feed_item(ui, item, feed_width);
                if action.focus_panel.is_some() {
                    result.focus_panel = action.focus_panel;
                }
                if let Some(id) = action.dismiss_id {
                    result.dismissed_ids.push(id);
                }
                if index + 1 < items.len() {
                    ui.add_space(FEED_ITEM_SPACING);
                }
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

    // The area itself claims background pointer input. Detect a card click from
    // the frame's hover response so this card does not sit on top of its own
    // dismiss and navigation buttons in egui's interaction order.
    if item.panel_id.is_some() && resp.contains_pointer() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if ui.input(|input| input.pointer.primary_clicked()) {
            return FeedItemAction {
                focus_panel: item.panel_id,
                ..FeedItemAction::default()
            };
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
    available_rect: Rect,
    minimap_height: f32,
    overlays: &OverlaysConfig,
    board: &Board,
) -> Option<Rect> {
    let now = std::time::SystemTime::now();
    if visible_attention_items(&board.attention, now).items.is_empty() {
        return None;
    }

    attention_feed_layout(available_rect, minimap_height, overlays).map(|layout| layout.outer_rect)
}

fn snapped_feed_list_height(feed_target_height: f32) -> f32 {
    let available = (feed_target_height - FEED_HEADER_BLOCK_HEIGHT).max(0.0);
    if available <= FEED_ITEM_VIEWPORT_HEIGHT {
        return available;
    }
    let row_span = FEED_ITEM_VIEWPORT_HEIGHT + FEED_ITEM_SPACING;
    let visible_rows = ((available + FEED_ITEM_SPACING) / row_span).floor().max(1.0);
    (visible_rows * FEED_ITEM_VIEWPORT_HEIGHT) + ((visible_rows - 1.0) * FEED_ITEM_SPACING)
}

fn attention_feed_layout(
    available_rect: Rect,
    minimap_height: f32,
    overlays: &OverlaysConfig,
) -> Option<AttentionFeedLayout> {
    let minimap_gap = if minimap_height > 0.0 { FEED_MINIMAP_GAP } else { 0.0 };
    let right = available_rect.max.x - FEED_MARGIN;
    let bottom = available_rect.max.y - FEED_MARGIN - minimap_height - minimap_gap;
    let left_limit = available_rect.min.x + FEED_MARGIN;
    let top_limit = available_rect.min.y + FEED_MARGIN;
    let max_outer_size = Vec2::new((right - left_limit).max(0.0), (bottom - top_limit).max(0.0));
    if max_outer_size.x <= FEED_FRAME_HORIZONTAL_INSET
        || max_outer_size.y <= FEED_FRAME_VERTICAL_INSET + FEED_HEADER_BLOCK_HEIGHT
    {
        return None;
    }

    let desired_outer_size = Vec2::new(
        overlays.attention_feed_width.max(FEED_MIN_WIDTH) + FEED_FRAME_HORIZONTAL_INSET,
        overlays.attention_feed_height.max(FEED_MIN_HEIGHT) + FEED_FRAME_VERTICAL_INSET,
    );
    let outer_size = desired_outer_size.min(max_outer_size);
    let content_size = Vec2::new(
        outer_size.x - FEED_FRAME_HORIZONTAL_INSET,
        outer_size.y - FEED_FRAME_VERTICAL_INSET,
    );
    let outer_rect = Rect::from_min_size(Pos2::new(right - outer_size.x, bottom - outer_size.y), outer_size);

    Some(AttentionFeedLayout {
        content_size,
        outer_rect,
    })
}

struct VisibleAttentionItems<'a> {
    items: Vec<&'a AttentionItem>,
    open_count: usize,
}

fn visible_attention_items(attention: &[AttentionItem], now: SystemTime) -> VisibleAttentionItems<'_> {
    let mut open: Vec<_> = attention.iter().filter(|item| item.is_open()).collect();
    let mut resolved: Vec<_> = attention
        .iter()
        .filter(|item| !item.is_open() && is_visible_attention_item(item, now))
        .collect();
    let open_count = open.len();
    open.sort_by_key(|item| std::cmp::Reverse((item.created_at, item.id.0)));
    resolved.sort_by_key(|item| {
        std::cmp::Reverse((item.resolved_at.unwrap_or(item.created_at), item.created_at, item.id.0))
    });
    resolved.truncate(MAX_RECENT_RESOLVED_ITEMS);
    open.extend(resolved);

    VisibleAttentionItems {
        items: open,
        open_count,
    }
}

pub(super) fn is_visible_attention_item(item: &AttentionItem, now: SystemTime) -> bool {
    if item.is_open() {
        return true;
    }

    item.is_resolved()
        && item
            .resolved_at
            .and_then(|resolved_at| now.duration_since(resolved_at).ok())
            .is_some_and(|elapsed| elapsed < RESOLVED_ATTENTION_RETENTION)
}

#[cfg(test)]
mod tests;
