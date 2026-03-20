use std::convert::TryFrom;
use std::time::Duration;

use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{DailyUsage, Panel, TaskUsageSummary, ToolUsage, UsageSnapshot, format_cost, format_tokens};

use crate::{loading_spinner, theme};

const CODEX_COLOR: Color32 = Color32::from_rgb(100, 200, 120);
const OPENCODE_COLOR: Color32 = Color32::from_rgb(102, 214, 173);
const SECTION_FONT_SIZE: f32 = 11.0;
const STAT_LABEL_SIZE: f32 = 12.0;
const STAT_VALUE_SIZE: f32 = 14.0;
const CARD_CORNER: u8 = 8;
const CARD_PAD: f32 = 12.0;
const BAR_HEIGHT: f32 = 10.0;
const BAR_MAX_WIDTH: f32 = 120.0;
const ROW_HEIGHT: f32 = 22.0;

pub struct UsageDashboardView<'a> {
    panel: &'a mut Panel,
    task_usage: &'a [TaskUsageSummary],
}

impl<'a> UsageDashboardView<'a> {
    pub fn new(panel: &'a mut Panel, task_usage: &'a [TaskUsageSummary]) -> Self {
        Self { panel, task_usage }
    }

    /// Renders the usage dashboard. Returns `true` if clicked (for focus tracking).
    pub fn show(&mut self, ui: &mut egui::Ui, _is_active_panel: bool) -> bool {
        let clicked = ui.rect_contains_pointer(ui.max_rect());

        // Poll for new data
        if let Some(dashboard) = self.panel.content.usage_mut() {
            dashboard.poll();
        }

        let snapshot = self.panel.content.usage().and_then(|d| d.snapshot.as_ref());

        ui.ctx().request_repaint_after(Duration::from_secs(5));

        ScrollArea::vertical()
            .id_salt(("usage_dashboard", self.panel.id.0))
            .show(ui, |ui| {
                ui.add_space(8.0);

                if let Some(snapshot) = snapshot {
                    render_today_section(ui, snapshot);
                    ui.add_space(12.0);
                    render_week_section(ui, snapshot);
                    if !self.task_usage.is_empty() {
                        ui.add_space(12.0);
                        render_task_section(ui, self.task_usage);
                    }
                    ui.add_space(12.0);
                    render_daily_chart(ui, &snapshot.daily);
                    ui.add_space(8.0);
                    render_footer(ui, snapshot);
                } else {
                    render_loading(ui);
                }

                ui.add_space(8.0);
            });

        clicked
    }
}

fn render_section_header(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label.to_uppercase())
            .size(SECTION_FONT_SIZE)
            .color(theme::ACCENT)
            .strong(),
    );
    ui.add_space(4.0);
}

fn render_today_section(ui: &mut egui::Ui, snapshot: &UsageSnapshot) {
    render_section_header(ui, "Today");

    let available_width = ui.available_width();
    let card_height = 86.0;

    if available_width >= 520.0 {
        let card_width = ((available_width - 16.0) / 3.0).max(130.0);
        ui.horizontal(|ui| {
            render_stat_card(
                ui,
                "Claude Code",
                &snapshot.claude,
                theme::ACCENT,
                true,
                card_width,
                card_height,
            );
            ui.add_space(8.0);
            render_stat_card(
                ui,
                "Codex CLI",
                &snapshot.codex,
                CODEX_COLOR,
                false,
                card_width,
                card_height,
            );
            ui.add_space(8.0);
            render_stat_card(
                ui,
                "OpenCode",
                &snapshot.opencode,
                OPENCODE_COLOR,
                false,
                card_width,
                card_height,
            );
        });
    } else {
        let card_width = available_width.max(140.0);
        render_stat_card(
            ui,
            "Claude Code",
            &snapshot.claude,
            theme::ACCENT,
            true,
            card_width,
            card_height,
        );
        ui.add_space(8.0);
        render_stat_card(
            ui,
            "Codex CLI",
            &snapshot.codex,
            CODEX_COLOR,
            false,
            card_width,
            card_height,
        );
        ui.add_space(8.0);
        render_stat_card(
            ui,
            "OpenCode",
            &snapshot.opencode,
            OPENCODE_COLOR,
            false,
            card_width,
            card_height,
        );
    }
}

fn render_stat_card(
    ui: &mut egui::Ui,
    title: &str,
    usage: &ToolUsage,
    title_color: Color32,
    show_messages: bool,
    width: f32,
    height: f32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(CARD_CORNER), theme::BG_ELEVATED);
    painter.rect_stroke(
        rect,
        CornerRadius::same(CARD_CORNER),
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
        egui::StrokeKind::Outside,
    );

    let x = rect.min.x + CARD_PAD;
    let mut y = rect.min.y + CARD_PAD;

    painter.text(
        Pos2::new(x, y),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(STAT_LABEL_SIZE),
        title_color,
    );
    y += 18.0;

    let sessions_text = format!("{} sessions", usage.today_sessions);
    painter.text(
        Pos2::new(x, y),
        egui::Align2::LEFT_TOP,
        &sessions_text,
        egui::FontId::proportional(STAT_VALUE_SIZE),
        theme::FG,
    );
    y += 17.0;

    let tokens_text = format!("{} tokens", format_tokens(usage.today_tokens));
    painter.text(
        Pos2::new(x, y),
        egui::Align2::LEFT_TOP,
        &tokens_text,
        egui::FontId::proportional(STAT_VALUE_SIZE),
        theme::FG_SOFT,
    );

    if show_messages && usage.today_messages > 0 {
        let msg_text = format!("{} messages", usage.today_messages);
        painter.text(
            Pos2::new(rect.max.x - CARD_PAD, y),
            egui::Align2::RIGHT_TOP,
            &msg_text,
            egui::FontId::proportional(STAT_LABEL_SIZE),
            theme::FG_DIM,
        );
    }

    if usage.today_cost > 0.0 {
        painter.text(
            Pos2::new(rect.max.x - CARD_PAD, rect.max.y - CARD_PAD),
            egui::Align2::RIGHT_BOTTOM,
            format_cost(usage.today_cost),
            egui::FontId::proportional(STAT_LABEL_SIZE),
            theme::FG_DIM,
        );
    }
}

fn render_week_section(ui: &mut egui::Ui, snapshot: &UsageSnapshot) {
    render_section_header(ui, "This Week");

    let claude_line = format!(
        "Claude: {} sessions \u{00B7} {} tokens{}",
        snapshot.claude.week_sessions,
        format_tokens(snapshot.claude.week_tokens),
        if snapshot.claude.week_messages > 0 {
            format!(" \u{00B7} {} msgs", snapshot.claude.week_messages)
        } else {
            String::new()
        },
    );
    ui.label(RichText::new(claude_line).size(STAT_VALUE_SIZE).color(theme::FG_SOFT));

    let codex_line = format!(
        "Codex:  {} sessions \u{00B7} {} tokens",
        snapshot.codex.week_sessions,
        format_tokens(snapshot.codex.week_tokens),
    );
    ui.label(RichText::new(codex_line).size(STAT_VALUE_SIZE).color(theme::FG_SOFT));

    let opencode_line = format!(
        "OpenCode: {} sessions \u{00B7} {} tokens{}",
        snapshot.opencode.week_sessions,
        format_tokens(snapshot.opencode.week_tokens),
        if snapshot.opencode.week_cost > 0.0 {
            format!(" \u{00B7} {}", format_cost(snapshot.opencode.week_cost))
        } else {
            String::new()
        },
    );
    ui.label(RichText::new(opencode_line).size(STAT_VALUE_SIZE).color(theme::FG_SOFT));
}

fn render_task_section(ui: &mut egui::Ui, task_usage: &[TaskUsageSummary]) {
    render_section_header(ui, "Tasks");

    for task in task_usage.iter().take(8) {
        let claude = if task.claude_sessions > 0 {
            format!(
                "Claude {}s / {} tok / {} msgs",
                task.claude_sessions,
                format_tokens(task.claude_tokens),
                task.claude_messages
            )
        } else {
            "Claude 0".to_string()
        };
        let codex = if task.codex_sessions > 0 {
            format!(
                "Codex {}s / {} tok",
                task.codex_sessions,
                format_tokens(task.codex_tokens)
            )
        } else {
            "Codex 0".to_string()
        };

        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(CARD_CORNER), theme::BG_ELEVATED);
        painter.rect_stroke(
            rect,
            CornerRadius::same(CARD_CORNER),
            egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
            egui::StrokeKind::Inside,
        );
        painter.text(
            Pos2::new(rect.min.x + CARD_PAD, rect.min.y + 10.0),
            egui::Align2::LEFT_TOP,
            &task.label,
            egui::FontId::proportional(12.0),
            theme::FG,
        );
        painter.text(
            Pos2::new(rect.min.x + CARD_PAD, rect.min.y + 26.0),
            egui::Align2::LEFT_TOP,
            format!("{claude}  ·  {codex}"),
            egui::FontId::monospace(10.0),
            theme::FG_DIM,
        );
        painter.text(
            Pos2::new(rect.max.x - CARD_PAD, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            format!("{} tok", format_tokens(task.total_tokens())),
            egui::FontId::monospace(11.0),
            theme::ACCENT,
        );
        ui.add_space(6.0);
    }
}

fn render_daily_chart(ui: &mut egui::Ui, daily: &[DailyUsage]) {
    if daily.is_empty() {
        return;
    }

    render_section_header(ui, "Last 14 Days");

    let max_sessions: u32 = daily
        .iter()
        .map(|d| {
            d.claude_sessions
                .saturating_add(d.codex_sessions)
                .saturating_add(d.opencode_sessions)
        })
        .max()
        .unwrap_or(1)
        .max(1);

    let available = ui.available_width();
    let date_width = 62.0;
    let bar_area_start = date_width + 8.0;
    let counts_width = 172.0;
    let bar_area_width = (available - bar_area_start - counts_width).clamp(40.0, BAR_MAX_WIDTH);

    render_daily_chart_header(ui, date_width, bar_area_width);

    for day in daily {
        render_daily_chart_row(ui, day, available, bar_area_start, bar_area_width, max_sessions);
    }
}

fn render_daily_chart_header(ui: &mut egui::Ui, date_width: f32, bar_area_width: f32) {
    ui.horizontal(|ui| {
        ui.add_space(date_width + 8.0 + bar_area_width + 4.0);
        ui.label(RichText::new("Claude").size(10.0).color(theme::ACCENT).strong());
        ui.add_space(8.0);
        ui.label(RichText::new("Codex").size(10.0).color(CODEX_COLOR).strong());
        ui.add_space(8.0);
        ui.label(RichText::new("OpenCode").size(10.0).color(OPENCODE_COLOR).strong());
    });
}

fn render_daily_chart_row(
    ui: &mut egui::Ui,
    day: &DailyUsage,
    available: f32,
    bar_area_start: f32,
    bar_area_width: f32,
    max_sessions: u32,
) {
    let (row_rect, _) = ui.allocate_exact_size(Vec2::new(available, ROW_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    let counts_x = row_rect.min.x + bar_area_start + bar_area_width + 8.0;

    paint_daily_chart_date(painter, &row_rect, day);
    paint_daily_chart_bar(painter, &row_rect, day, bar_area_start, bar_area_width, max_sessions);
    paint_daily_chart_count(painter, counts_x, row_rect.center().y, day.claude_sessions, 0.0);
    paint_daily_chart_count(painter, counts_x, row_rect.center().y, day.codex_sessions, 52.0);
    paint_daily_chart_count(painter, counts_x, row_rect.center().y, day.opencode_sessions, 112.0);
}

fn paint_daily_chart_date(painter: &egui::Painter, row_rect: &Rect, day: &DailyUsage) {
    painter.text(
        Pos2::new(row_rect.min.x + 2.0, row_rect.center().y),
        egui::Align2::LEFT_CENTER,
        format_short_date(&day.date),
        egui::FontId::monospace(11.0),
        theme::FG_DIM,
    );
}

fn paint_daily_chart_bar(
    painter: &egui::Painter,
    row_rect: &Rect,
    day: &DailyUsage,
    bar_area_start: f32,
    bar_area_width: f32,
    max_sessions: u32,
) {
    let total = day
        .claude_sessions
        .saturating_add(day.codex_sessions)
        .saturating_add(day.opencode_sessions);
    if total == 0 {
        return;
    }

    let total_width = scaled_bar_width(bar_area_width, total, max_sessions);
    let claude_width = scaled_bar_width(total_width, day.claude_sessions, total);
    let remaining_width = (total_width - claude_width).max(0.0);
    let codex_width = scaled_bar_width(
        remaining_width,
        day.codex_sessions,
        total.saturating_sub(day.claude_sessions),
    );
    let opencode_width = (remaining_width - codex_width).max(0.0);
    let bar_x = row_rect.min.x + bar_area_start;
    let bar_y = row_rect.center().y - BAR_HEIGHT / 2.0;

    paint_daily_chart_bar_segment(painter, bar_x, bar_y, claude_width, theme::ACCENT);
    paint_daily_chart_bar_segment(painter, bar_x + claude_width, bar_y, codex_width, CODEX_COLOR);
    paint_daily_chart_bar_segment(
        painter,
        bar_x + claude_width + codex_width,
        bar_y,
        opencode_width,
        OPENCODE_COLOR,
    );
}

fn paint_daily_chart_bar_segment(painter: &egui::Painter, x: f32, y: f32, width: f32, color: Color32) {
    if width <= 0.0 {
        return;
    }

    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, BAR_HEIGHT)),
        CornerRadius::same(3),
        color,
    );
}

fn paint_daily_chart_count(painter: &egui::Painter, counts_x: f32, center_y: f32, sessions: u32, offset_x: f32) {
    painter.text(
        Pos2::new(counts_x + offset_x, center_y),
        egui::Align2::LEFT_CENTER,
        sessions.to_string(),
        egui::FontId::monospace(11.0),
        if sessions > 0 { theme::FG_SOFT } else { theme::FG_DIM },
    );
}

fn scaled_bar_width(bar_area_width: f32, numerator: u32, denominator: u32) -> f32 {
    if denominator == 0 {
        return 0.0;
    }

    // Session counts stay small in practice; clamp pathological values so the
    // egui width math can use exact `u16 -> f32` conversions.
    let numerator = bounded_session_count(numerator);
    let denominator = bounded_session_count(denominator);

    (bar_area_width * (f32::from(numerator) / f32::from(denominator))).clamp(0.0, bar_area_width)
}

fn bounded_session_count(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn render_footer(ui: &mut egui::Ui, snapshot: &UsageSnapshot) {
    let elapsed = snapshot.updated_at.elapsed();
    let ago = if elapsed.as_secs() < 2 {
        "just now".to_string()
    } else if elapsed.as_secs() < 60 {
        format!("{}s ago", elapsed.as_secs())
    } else {
        format!("{}m ago", elapsed.as_secs() / 60)
    };

    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.label(
            RichText::new(format!("Last updated: {ago}"))
                .size(10.0)
                .color(theme::FG_DIM),
        );
    });
}

fn render_loading(ui: &mut egui::Ui) {
    loading_spinner::show(
        ui,
        egui::Id::new("usage_loading_spinner"),
        Some("Loading usage data\u{2026}"),
    );
}

/// Convert "2026-03-16" to "Mar 16".
fn format_short_date(date: &str) -> String {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return date.to_string();
    }
    let month = match parts[1] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => parts[1],
    };
    let day = parts[2].trim_start_matches('0');
    format!("{month} {day:>2}")
}

#[cfg(test)]
mod tests {
    use super::format_short_date;

    #[test]
    fn short_date_formatting() {
        assert_eq!(format_short_date("2026-03-16"), "Mar 16");
        assert_eq!(format_short_date("2026-01-05"), "Jan  5");
        assert_eq!(format_short_date("2026-12-25"), "Dec 25");
    }
}
