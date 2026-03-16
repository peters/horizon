use egui::{Color32, Id, Order, RichText, ScrollArea, Vec2};
use horizon_core::{AttentionItem, AttentionSeverity, Board};
use std::time::SystemTime;

use crate::theme;

const FEED_WIDTH: f32 = 320.0;
const FEED_MAX_HEIGHT: f32 = 240.0;
const FEED_MARGIN: f32 = 16.0;
const FEED_ITEM_SPACING: f32 = 4.0;

pub fn render_attention_feed(ctx: &egui::Context, board: &Board, minimap_height: f32) {
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
        return;
    }

    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(10);

    let offset_y = FEED_MARGIN + minimap_height + if minimap_height > 0.0 { 8.0 } else { 0.0 };

    egui::Area::new(Id::new("attention_feed"))
        .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-FEED_MARGIN, -offset_y))
        .order(Order::Foreground)
        .interactable(false)
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
                ui.set_width(FEED_WIDTH);
                render_feed_header(ui, &items);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);
                render_feed_list(ui, &items);
            });
        });
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

fn render_feed_list(ui: &mut egui::Ui, items: &[&AttentionItem]) {
    ScrollArea::vertical()
        .max_height(FEED_MAX_HEIGHT - 40.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in items {
                render_feed_item(ui, item);
                ui.add_space(FEED_ITEM_SPACING);
            }
        });
}

fn render_feed_item(ui: &mut egui::Ui, item: &AttentionItem) {
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

    frame.show(ui, |ui| {
        ui.set_width(FEED_WIDTH - 16.0);
        render_feed_item_header(ui, item, is_resolved, color);
        let msg_color = if is_resolved {
            theme::alpha(theme::FG_SOFT, 100)
        } else {
            theme::FG_SOFT
        };
        ui.label(RichText::new(&item.summary).size(11.0).color(msg_color));
    });
}

fn render_feed_item_header(ui: &mut egui::Ui, item: &AttentionItem, is_resolved: bool, color: Color32) {
    ui.horizontal(|ui| {
        let dot_rect = ui.allocate_space(Vec2::new(6.0, 6.0));
        let dot_color = if is_resolved { theme::alpha(color, 100) } else { color };
        ui.painter().circle_filled(dot_rect.1.center(), 3.0, dot_color);

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
