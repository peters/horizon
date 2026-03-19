use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use horizon_core::{RemoteHost, RemoteHostStatus};

use super::layout::{Columns, HEADER_ROW_HEIGHT, ROW_HEIGHT};
use crate::theme;

pub(super) fn render_column_headers(ui: &mut Ui, width: f32, columns: &Columns) {
    let rect = ui.allocate_space(Vec2::new(width, HEADER_ROW_HEIGHT)).1;
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    let x = rect.min.x;
    let font = FontId::monospace(10.0);
    let color = theme::FG_DIM;
    let underline_stroke = Stroke::new(1.0, theme::alpha(theme::ACCENT, 40));
    let headers = [
        ("Alias", columns.alias),
        ("IPv4", columns.ipv4),
        ("Tags", columns.tags),
        ("Hostname", columns.hostname),
        ("On", columns.status),
        ("Last seen", columns.last_seen),
    ];

    for (label, column_x) in headers {
        let text_rect = painter.text(
            Pos2::new(x + column_x, y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        painter.line_segment(
            [
                Pos2::new(text_rect.min.x, text_rect.max.y + 1.0),
                Pos2::new(text_rect.max.x, text_rect.max.y + 1.0),
            ],
            underline_stroke,
        );
    }
}

pub(super) fn render_host_row(
    ui: &mut Ui,
    width: f32,
    index: usize,
    host: &RemoteHost,
    is_selected: bool,
    columns: &Columns,
    now_secs: i64,
) -> bool {
    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(4),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("rh_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(4),
                theme::alpha(theme::PANEL_BG_ALT, 120),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("rh_click", index)), Sense::click());
    if click.double_clicked() || (click.clicked() && is_selected) {
        clicked = true;
    }

    let painter = ui.painter_at(row_rect);
    let y = row_rect.center().y;
    let x = row_rect.min.x;
    let mono = FontId::monospace(12.0);
    let mono_sm = FontId::monospace(11.0);

    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(x + 4.0, row_rect.min.y + 5.0),
            Vec2::new(3.0, ROW_HEIGHT - 10.0),
        ),
        CornerRadius::same(2),
        status_color(host.status),
    );

    painter.text(
        Pos2::new(x + columns.alias, y),
        egui::Align2::LEFT_CENTER,
        &host.label,
        mono.clone(),
        alias_color(host.status),
    );

    let ip = host.ips.first().map_or("-", String::as_str);
    painter.text(
        Pos2::new(x + columns.ipv4, y),
        egui::Align2::LEFT_CENTER,
        ip,
        mono_sm.clone(),
        theme::FG_SOFT,
    );

    render_tags(
        &painter,
        Pos2::new(x + columns.tags, y),
        &host.tags,
        &mono_sm,
        x + columns.hostname - 12.0,
    );

    let hostname = host.hostname.as_deref().unwrap_or("-");
    painter.text(
        Pos2::new(x + columns.hostname, y),
        egui::Align2::LEFT_CENTER,
        hostname,
        mono_sm.clone(),
        theme::FG_DIM,
    );

    let (status_text, status_text_color) = status_text(host.status);
    painter.text(
        Pos2::new(x + columns.status, y),
        egui::Align2::LEFT_CENTER,
        status_text,
        mono_sm.clone(),
        status_text_color,
    );

    let last_seen_display = host
        .last_seen_secs
        .map_or_else(|| "-".to_string(), |secs| format_relative_time(secs, now_secs));
    painter.text(
        Pos2::new(x + columns.last_seen, y),
        egui::Align2::LEFT_CENTER,
        &last_seen_display,
        mono_sm,
        theme::FG_DIM,
    );

    clicked
}

pub(super) fn paint_empty(ui: &mut Ui, message: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(message).color(theme::FG_DIM).size(13.0));
    });
}

fn render_tags(painter: &egui::Painter, pos: Pos2, tags: &[String], font: &FontId, max_x: f32) {
    let mut x = pos.x;
    for (index, tag) in tags.iter().enumerate() {
        if x >= max_x {
            break;
        }
        if index > 0 {
            let comma_rect = painter.text(
                Pos2::new(x, pos.y),
                egui::Align2::LEFT_CENTER,
                ",",
                font.clone(),
                theme::FG_DIM,
            );
            x = comma_rect.max.x;
        }
        let text_rect = painter.text(
            Pos2::new(x, pos.y),
            egui::Align2::LEFT_CENTER,
            tag,
            font.clone(),
            tag_color(tag),
        );
        x = text_rect.max.x;
    }
}

fn format_relative_time(epoch_secs: i64, now_secs: i64) -> String {
    let delta = now_secs.saturating_sub(epoch_secs);

    if delta < 0 {
        "-".to_string()
    } else if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{} min ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

const TAG_COLORS: [Color32; 8] = [
    Color32::from_rgb(249, 226, 175),
    Color32::from_rgb(137, 180, 250),
    Color32::from_rgb(166, 227, 161),
    Color32::from_rgb(243, 139, 168),
    Color32::from_rgb(202, 151, 234),
    Color32::from_rgb(102, 212, 214),
    Color32::from_rgb(233, 190, 109),
    Color32::from_rgb(147, 187, 255),
];

fn tag_color(tag: &str) -> Color32 {
    let hash = tag
        .bytes()
        .fold(0_u32, |acc, byte| acc.wrapping_mul(31).wrapping_add(u32::from(byte)));
    TAG_COLORS[(hash as usize) % TAG_COLORS.len()]
}

fn status_color(status: RemoteHostStatus) -> Color32 {
    match status {
        RemoteHostStatus::Online => theme::PALETTE_GREEN,
        RemoteHostStatus::Offline => theme::PALETTE_RED,
        RemoteHostStatus::Unknown => theme::FG_DIM,
    }
}

fn alias_color(status: RemoteHostStatus) -> Color32 {
    match status {
        RemoteHostStatus::Online => Color32::from_rgb(166, 227, 161),
        RemoteHostStatus::Offline => Color32::from_rgb(249, 226, 175),
        RemoteHostStatus::Unknown => theme::FG_SOFT,
    }
}

fn status_text(status: RemoteHostStatus) -> (&'static str, Color32) {
    match status {
        RemoteHostStatus::Online => ("yes", theme::PALETTE_GREEN),
        RemoteHostStatus::Offline => ("no", theme::PALETTE_RED),
        RemoteHostStatus::Unknown => ("-", theme::FG_DIM),
    }
}
