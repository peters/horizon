use egui::{Color32, CornerRadius, Id, Order, RichText, ScrollArea, Sense, Vec2};
use horizon_core::{AttentionItem, AttentionSeverity, Board, OverlaysConfig, PanelId};
use std::time::SystemTime;

use crate::theme;

const FEED_MIN_WIDTH: f32 = 240.0;
const FEED_MARGIN: f32 = 16.0;
const FEED_ITEM_SPACING: f32 = 4.0;
const FEED_HEADER_BLOCK_HEIGHT: f32 = 30.0;
const FEED_ITEM_VIEWPORT_HEIGHT: f32 = 74.0;

/// Renders the attention feed overlay. Returns a panel ID if the user clicked
/// an item to navigate to it.
pub fn render_attention_feed(
    ctx: &egui::Context,
    board: &Board,
    minimap_height: f32,
    overlays: &OverlaysConfig,
) -> Option<PanelId> {
    let feed_width = overlays.attention_feed_width.max(FEED_MIN_WIDTH);
    let now = SystemTime::now();
    let mut items: Vec<&AttentionItem> = board
        .attention
        .iter()
        .filter(|item| {
            if item.is_open() {
                return true;
            }
            if let Some(resolved_at) = item.resolved_at
                && let Ok(elapsed) = now.duration_since(resolved_at)
            {
                return elapsed.as_secs() < 30;
            }
            false
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(10);

    let offset_y = FEED_MARGIN + minimap_height + if minimap_height > 0.0 { 8.0 } else { 0.0 };
    let feed_target_height = overlays
        .attention_feed_height
        .max(FEED_HEADER_BLOCK_HEIGHT + FEED_ITEM_VIEWPORT_HEIGHT);
    let mut clicked_panel: Option<PanelId> = None;
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
                .fill(theme::BG_ELEVATED)
                .stroke(egui::Stroke::new(
                    1.0,
                    theme::blend(theme::BG_ELEVATED, theme::FG_DIM, 0.15),
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
                clicked_panel = render_feed_list(ui, &items, feed_width, list_max_height, scroll_id);
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

    clicked_panel
}

fn render_feed_header(ui: &mut egui::Ui, items: &[&AttentionItem]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Attention Feed").size(11.0).color(theme::FG_DIM).strong());
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
) -> Option<PanelId> {
    let mut clicked = None;
    ScrollArea::vertical()
        .id_salt(scroll_id)
        .max_height(list_max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in items {
                if let Some(panel_id) = render_feed_item(ui, item, feed_width) {
                    clicked = Some(panel_id);
                }
                ui.add_space(FEED_ITEM_SPACING);
            }
        });
    clicked
}

fn render_feed_item(ui: &mut egui::Ui, item: &AttentionItem, feed_width: f32) -> Option<PanelId> {
    let is_resolved = !item.is_open();
    let color = severity_color(item.severity);
    let bg_color = Color32::from_rgba_premultiplied(
        color.r() / 8,
        color.g() / 8,
        color.b() / 8,
        if is_resolved { 20 } else { 40 },
    );

    let frame = egui::Frame::new()
        .fill(bg_color)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 5));

    let resp = frame
        .show(ui, |ui| {
            ui.set_width(feed_width - 16.0);
            render_feed_item_header(ui, item, is_resolved, color);
            let msg_color = if is_resolved {
                theme::alpha(theme::FG_SOFT, 100)
            } else {
                theme::FG_SOFT
            };
            ui.label(RichText::new(&item.summary).size(11.0).color(msg_color));

            if item.is_open() && item.panel_id.is_some() {
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Button::new(RichText::new("Go to panel \u{2192}").size(9.5).color(theme::ACCENT))
                                .fill(theme::alpha(theme::ACCENT, 20))
                                .corner_radius(CornerRadius::same(4))
                                .stroke(egui::Stroke::NONE),
                        );
                    });
                });
            }
        })
        .response;

    // Make the whole item clickable to navigate
    if item.panel_id.is_some() {
        let interact = ui.interact(resp.rect, Id::new(("attn_item", item.id.0)), Sense::click());
        if interact.clicked() {
            return item.panel_id;
        }
        if interact.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    None
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
                .color(theme::alpha(theme::FG_DIM, if is_resolved { 100 } else { 180 })),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let elapsed = format_elapsed(item.created_at);
            ui.label(
                RichText::new(elapsed)
                    .size(9.0)
                    .color(theme::alpha(theme::FG_DIM, if is_resolved { 80 } else { 140 })),
            );
            if is_resolved {
                ui.label(
                    RichText::new("\u{2713}")
                        .size(9.0)
                        .color(theme::alpha(theme::PALETTE_GREEN, 120)),
                );
            }
        });
    });
}

fn severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED,
        AttentionSeverity::Medium => theme::PALETTE_GREEN,
        AttentionSeverity::Low => theme::ACCENT,
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

fn snapped_feed_list_height(feed_target_height: f32) -> f32 {
    let available = (feed_target_height - FEED_HEADER_BLOCK_HEIGHT).max(FEED_ITEM_VIEWPORT_HEIGHT);
    let row_span = FEED_ITEM_VIEWPORT_HEIGHT + FEED_ITEM_SPACING;
    let visible_rows = ((available + FEED_ITEM_SPACING) / row_span).floor().max(1.0);
    (visible_rows * FEED_ITEM_VIEWPORT_HEIGHT) + ((visible_rows - 1.0) * FEED_ITEM_SPACING)
}
