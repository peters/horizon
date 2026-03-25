use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, TextFormat, Ui, Vec2,
};
use horizon_core::{
    RemoteHost, RemoteHostConnectionHistoryEntry, RemoteHostConnectionSummary, RemoteHostStatus, SshConnectionStatus,
};

use super::layout::{Columns, HEADER_ROW_HEIGHT, ROW_HEIGHT};
use crate::theme;

const COLUMN_GUTTER: f32 = 18.0;
const HISTORY_PREVIEW_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct HostRowInteraction {
    pub(super) select: bool,
    pub(super) connect: bool,
    pub(super) toggle_expand: bool,
}

pub(super) struct HostRowRenderContext<'a> {
    pub(super) width: f32,
    pub(super) index: usize,
    pub(super) host: &'a RemoteHost,
    pub(super) summary: &'a RemoteHostConnectionSummary,
    pub(super) is_selected: bool,
    pub(super) is_expanded: bool,
    pub(super) columns: &'a Columns,
    pub(super) now_secs: i64,
}

struct HostRowLayout {
    row: Rect,
    chevron: Rect,
    body: Rect,
}

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

pub(super) fn render_host_row(ui: &mut Ui, row: &HostRowRenderContext<'_>) -> HostRowInteraction {
    let layout = host_row_layout(ui, row.width, row.columns);
    let interaction = host_row_interaction(ui, &layout, row.index, row.is_selected);

    paint_row_background(ui, layout.row, interaction, row.is_selected);
    paint_host_row_contents(ui, &layout, row);

    interaction
}

pub(super) fn render_host_details(
    ui: &mut Ui,
    width: f32,
    host: &RemoteHost,
    summary: &RemoteHostConnectionSummary,
    now_secs: i64,
) {
    egui::Frame::new()
        .fill(theme::alpha(theme::PANEL_BG_ALT, 150))
        .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 160)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                render_status_badge(ui, summary.current_status());
                ui.label(
                    RichText::new(host.display_target())
                        .font(FontId::monospace(11.5))
                        .color(theme::FG_SOFT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(session_count_text(summary))
                            .font(FontId::monospace(10.5))
                            .color(theme::FG_DIM),
                    );
                });
            });

            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_meta_badge(ui, format!("source {}", host.sources.label()), theme::ACCENT);
                render_meta_badge(
                    ui,
                    format!("network {}", host.status.label()),
                    status_color(host.status),
                );
                if let Some(hostname) = host.hostname.as_deref() {
                    render_meta_badge(ui, hostname.to_string(), theme::FG_SOFT);
                }
                if let Some(os) = host.os.as_deref() {
                    render_meta_badge(ui, os.to_string(), theme::FG_DIM);
                }
            });

            ui.add_space(8.0);
            ui.label(
                RichText::new("Connection history")
                    .font(FontId::monospace(10.5))
                    .color(theme::FG_DIM),
            );
            ui.add_space(4.0);

            if summary.history.is_empty() {
                ui.label(
                    RichText::new("No SSH sessions opened from this board yet.")
                        .font(FontId::monospace(10.5))
                        .color(theme::FG_DIM),
                );
                return;
            }

            for entry in summary.history.iter().take(HISTORY_PREVIEW_LIMIT) {
                render_history_entry(ui, entry, now_secs);
            }

            let remaining = summary.history.len().saturating_sub(HISTORY_PREVIEW_LIMIT);
            if remaining > 0 {
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!("+{remaining} older session(s)"))
                        .font(FontId::monospace(10.5))
                        .color(theme::FG_DIM),
                );
            }
        });
}

pub(super) fn paint_empty(ui: &mut Ui, message: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(message).color(theme::FG_DIM).size(13.0));
    });
}

fn render_tags(painter: &egui::Painter, pos: Pos2, tags: &[String], font: &FontId, max_x: f32) {
    if tags.is_empty() {
        return;
    }

    render_layout_job(painter, pos, tags_layout_job(tags, font, max_x - pos.x));
}

fn host_row_layout(ui: &mut Ui, width: f32, columns: &Columns) -> HostRowLayout {
    let row = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let chevron = Rect::from_min_max(
        Pos2::new(row.min.x + columns.alias - 4.0, row.min.y),
        Pos2::new(row.min.x + columns.alias + 12.0, row.max.y),
    );

    HostRowLayout {
        body: Rect::from_min_max(Pos2::new(chevron.max.x + 4.0, row.min.y), row.max),
        row,
        chevron,
    }
}

fn host_row_interaction(ui: &mut Ui, layout: &HostRowLayout, index: usize, is_selected: bool) -> HostRowInteraction {
    let chevron_response = ui.interact(
        layout.chevron,
        ui.make_persistent_id(("rh_expand", index)),
        Sense::click(),
    );
    let body_response = ui.interact(layout.body, ui.make_persistent_id(("rh_click", index)), Sense::click());

    HostRowInteraction {
        select: chevron_response.clicked() || body_response.clicked(),
        connect: !chevron_response.clicked()
            && (body_response.double_clicked() || (body_response.clicked() && is_selected)),
        toggle_expand: chevron_response.clicked(),
    }
}

fn paint_row_background(ui: &mut Ui, row_rect: Rect, interaction: HostRowInteraction, is_selected: bool) {
    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(4),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
        return;
    }

    if interaction.select || ui.rect_contains_pointer(row_rect) {
        ui.painter_at(row_rect)
            .rect_filled(row_rect, CornerRadius::same(4), theme::alpha(theme::PANEL_BG_ALT, 120));
    }
}

fn paint_host_row_contents(ui: &mut Ui, layout: &HostRowLayout, row: &HostRowRenderContext<'_>) {
    let painter = ui.painter_at(layout.row);
    let y = layout.row.center().y;
    let x = layout.row.min.x;
    let mono = FontId::monospace(12.0);
    let mono_sm = FontId::monospace(11.0);

    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(x + 4.0, layout.row.min.y + 5.0),
            Vec2::new(3.0, ROW_HEIGHT - 10.0),
        ),
        CornerRadius::same(2),
        status_color(row.host.status),
    );
    painter.text(
        layout.chevron.center(),
        egui::Align2::CENTER_CENTER,
        if row.is_expanded { "v" } else { ">" },
        mono_sm.clone(),
        if row.summary.total_sessions() > 0 || row.is_expanded {
            theme::FG_SOFT
        } else {
            theme::FG_DIM
        },
    );
    painter.text(
        Pos2::new(layout.body.min.x, y),
        egui::Align2::LEFT_CENTER,
        &row.host.label,
        mono.clone(),
        alias_color(row.host.status),
    );

    paint_row_columns(&painter, x, y, &mono_sm, row);
}

fn paint_row_columns(painter: &egui::Painter, x: f32, y: f32, mono_sm: &FontId, row: &HostRowRenderContext<'_>) {
    let ip = row.host.ips.first().map_or("-", String::as_str);
    painter.text(
        Pos2::new(x + row.columns.ipv4, y),
        egui::Align2::LEFT_CENTER,
        ip,
        mono_sm.clone(),
        theme::FG_SOFT,
    );

    render_tags(
        painter,
        Pos2::new(x + row.columns.tags, y),
        &row.host.tags,
        mono_sm,
        x + row.columns.hostname - COLUMN_GUTTER,
    );

    let hostname = row.host.hostname.as_deref().unwrap_or("-");
    render_truncated_text(
        painter,
        Pos2::new(x + row.columns.hostname, y),
        hostname,
        mono_sm,
        theme::FG_DIM,
        x + row.columns.status - COLUMN_GUTTER,
    );

    let (network_status_text, network_status_color) = status_text(row.host.status);
    painter.text(
        Pos2::new(x + row.columns.status, y),
        egui::Align2::LEFT_CENTER,
        network_status_text,
        mono_sm.clone(),
        network_status_color,
    );

    let last_seen_display = row
        .host
        .last_seen_secs
        .map_or_else(|| "-".to_string(), |secs| format_relative_time(secs, row.now_secs));
    painter.text(
        Pos2::new(x + row.columns.last_seen, y),
        egui::Align2::LEFT_CENTER,
        &last_seen_display,
        mono_sm.clone(),
        theme::FG_DIM,
    );
}

fn render_status_badge(ui: &mut Ui, status: Option<SshConnectionStatus>) {
    let (label, color) = match status {
        Some(SshConnectionStatus::Connecting) => ("Connecting", Color32::from_rgb(249, 226, 175)),
        Some(SshConnectionStatus::Connected) => ("Connected", Color32::from_rgb(166, 227, 161)),
        Some(SshConnectionStatus::Disconnected) => ("Disconnected", theme::PALETTE_RED),
        None => ("No live session", theme::FG_DIM),
    };

    egui::Frame::new()
        .fill(theme::alpha(color, 24))
        .stroke(Stroke::new(1.0, theme::alpha(color, 120)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(label).font(FontId::monospace(10.0)).color(color));
        });
}

fn render_meta_badge(ui: &mut Ui, text: String, color: Color32) {
    egui::Frame::new()
        .fill(theme::alpha(theme::BG_ELEVATED, 170))
        .stroke(Stroke::new(1.0, theme::alpha(color, 70)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(FontId::monospace(10.0)).color(color));
        });
}

fn render_history_entry(ui: &mut Ui, entry: &RemoteHostConnectionHistoryEntry, now_secs: i64) {
    ui.horizontal(|ui| {
        let status_color = match entry.status {
            SshConnectionStatus::Connecting => Color32::from_rgb(249, 226, 175),
            SshConnectionStatus::Connected => Color32::from_rgb(166, 227, 161),
            SshConnectionStatus::Disconnected => theme::PALETTE_RED,
        };
        ui.colored_label(status_color, "•");
        ui.label(
            RichText::new(format_relative_millis(entry.launched_at_millis, now_secs))
                .font(FontId::monospace(10.5))
                .color(theme::FG_DIM),
        );
        ui.label(
            RichText::new(&entry.panel_title)
                .font(FontId::monospace(10.5))
                .color(theme::FG_SOFT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(&entry.workspace_name)
                    .font(FontId::monospace(10.0))
                    .color(theme::FG_DIM),
            );
        });
    });
}

fn render_truncated_text(painter: &egui::Painter, pos: Pos2, text: &str, font: &FontId, color: Color32, max_x: f32) {
    if text.is_empty() {
        return;
    }

    let mut job = single_line_job(max_x - pos.x);
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        },
    );
    render_layout_job(painter, pos, job);
}

fn render_layout_job(painter: &egui::Painter, pos: Pos2, job: LayoutJob) {
    let max_width = job.wrap.max_width.max(0.0);
    if max_width <= 0.0 || job.text.is_empty() {
        return;
    }

    let galley = painter.layout_job(job);
    let text_pos = Pos2::new(pos.x, pos.y - galley.size().y * 0.5);
    painter.galley(text_pos, galley, Color32::TRANSPARENT);
}

fn single_line_job(max_width: f32) -> LayoutJob {
    LayoutJob {
        break_on_newline: false,
        wrap: TextWrapping {
            max_width: max_width.max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        },
        ..Default::default()
    }
}

fn tags_layout_job(tags: &[String], font: &FontId, max_width: f32) -> LayoutJob {
    let mut job = single_line_job(max_width);

    for (index, tag) in tags.iter().enumerate() {
        if index > 0 {
            job.append(
                ",",
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: theme::FG_DIM,
                    ..Default::default()
                },
            );
        }
        job.append(
            tag,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: tag_color(tag),
                ..Default::default()
            },
        );
    }

    job
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

fn format_relative_millis(epoch_millis: i64, now_secs: i64) -> String {
    if epoch_millis <= 0 {
        return "-".to_string();
    }

    format_relative_time(epoch_millis / 1000, now_secs)
}

fn session_count_text(summary: &RemoteHostConnectionSummary) -> String {
    match (summary.live_sessions(), summary.total_sessions()) {
        (0, 0) => "0 sessions".to_string(),
        (live, total) => format!("{live} live / {total} total"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_layout_job_uses_single_line_ellipsis_and_preserves_tag_colors() {
        let tags = vec!["tag:cuda".to_string(), "tag:node".to_string()];
        let font = FontId::monospace(11.0);

        let job = tags_layout_job(&tags, &font, 120.0);

        assert_eq!(job.text, "tag:cuda,tag:node");
        assert_eq!(job.sections.len(), 3);
        assert_eq!(job.sections[0].format.color, tag_color("tag:cuda"));
        assert_eq!(job.sections[1].format.color, theme::FG_DIM);
        assert_eq!(job.sections[2].format.color, tag_color("tag:node"));
        assert!(!job.break_on_newline);
        assert!((job.wrap.max_width - 120.0).abs() < f32::EPSILON);
        assert_eq!(job.wrap.max_rows, 1);
        assert!(job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, Some('\u{2026}'));
    }

    #[test]
    fn single_line_job_enables_ellipsis_wrapping() {
        let job = single_line_job(96.0);

        assert!(!job.break_on_newline);
        assert!((job.wrap.max_width - 96.0).abs() < f32::EPSILON);
        assert_eq!(job.wrap.max_rows, 1);
        assert!(job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, Some('\u{2026}'));
    }
}
