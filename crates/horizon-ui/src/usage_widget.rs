use std::convert::TryFrom;
use std::time::Duration;

use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, RichText, ScrollArea, Vec2};
use horizon_core::{DailyUsage, Panel, ToolUsage, UsageSnapshot, format_tokens};

use crate::theme;

const CODEX_COLOR: Color32 = Color32::from_rgb(100, 200, 120);
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
}

impl<'a> UsageDashboardView<'a> {
    pub fn new(panel: &'a mut Panel) -> Self {
        Self { panel }
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
    let card_width = ((available_width - 8.0) / 2.0).max(140.0);
    let card_height = 80.0;

    ui.horizontal(|ui| {
        render_stat_card(ui, "Claude Code", &snapshot.claude, true, card_width, card_height);
        ui.add_space(8.0);
        render_stat_card(ui, "Codex CLI", &snapshot.codex, false, card_width, card_height);
    });
}

fn render_stat_card(ui: &mut egui::Ui, title: &str, usage: &ToolUsage, is_claude: bool, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, CornerRadius::same(CARD_CORNER), theme::BG_ELEVATED);
    painter.rect_stroke(
        rect,
        CornerRadius::same(CARD_CORNER),
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
        egui::StrokeKind::Outside,
    );

    let title_color = if is_claude { theme::ACCENT } else { CODEX_COLOR };
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

    if is_claude && usage.today_messages > 0 {
        let msg_text = format!("{} messages", usage.today_messages);
        painter.text(
            Pos2::new(rect.max.x - CARD_PAD, y),
            egui::Align2::RIGHT_TOP,
            &msg_text,
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
}

fn render_daily_chart(ui: &mut egui::Ui, daily: &[DailyUsage]) {
    if daily.is_empty() {
        return;
    }

    render_section_header(ui, "Last 14 Days");

    // Find the max combined session count for scaling the bars.
    let max_sessions: u32 = daily
        .iter()
        .map(|d| d.claude_sessions.saturating_add(d.codex_sessions))
        .max()
        .unwrap_or(1)
        .max(1);

    // Header row
    let available = ui.available_width();
    let date_width = 62.0;
    let bar_area_start = date_width + 8.0;
    let counts_width = 120.0;
    let bar_area_width = (available - bar_area_start - counts_width).clamp(40.0, BAR_MAX_WIDTH);

    ui.horizontal(|ui| {
        ui.add_space(date_width + 8.0 + bar_area_width + 4.0);
        ui.label(RichText::new("Claude").size(10.0).color(theme::ACCENT).strong());
        ui.add_space(8.0);
        ui.label(RichText::new("Codex").size(10.0).color(CODEX_COLOR).strong());
    });

    for day in daily {
        let (row_rect, _) = ui.allocate_exact_size(Vec2::new(available, ROW_HEIGHT), egui::Sense::hover());
        let painter = ui.painter();

        // Date label (e.g. "Mar 16")
        let short_date = format_short_date(&day.date);
        painter.text(
            Pos2::new(row_rect.min.x + 2.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &short_date,
            egui::FontId::monospace(11.0),
            theme::FG_DIM,
        );

        // Stacked bar
        let bar_x = row_rect.min.x + bar_area_start;
        let bar_y = row_rect.center().y - BAR_HEIGHT / 2.0;
        let total = day.claude_sessions.saturating_add(day.codex_sessions);

        if total > 0 {
            let total_width = scaled_bar_width(bar_area_width, total, max_sessions);
            let claude_width = scaled_bar_width(total_width, day.claude_sessions, total);
            let codex_width = (total_width - claude_width).max(0.0);

            if claude_width > 0.0 {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(bar_x, bar_y), Vec2::new(claude_width, BAR_HEIGHT)),
                    CornerRadius::same(3),
                    theme::ACCENT,
                );
            }
            if codex_width > 0.0 {
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(bar_x + claude_width, bar_y),
                        Vec2::new(codex_width, BAR_HEIGHT),
                    ),
                    CornerRadius::same(3),
                    CODEX_COLOR,
                );
            }
        }

        // Session counts
        let counts_x = row_rect.min.x + bar_area_start + bar_area_width + 8.0;
        painter.text(
            Pos2::new(counts_x, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            day.claude_sessions.to_string(),
            egui::FontId::monospace(11.0),
            if day.claude_sessions > 0 {
                theme::FG_SOFT
            } else {
                theme::FG_DIM
            },
        );
        painter.text(
            Pos2::new(counts_x + 52.0, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            day.codex_sessions.to_string(),
            egui::FontId::monospace(11.0),
            if day.codex_sessions > 0 {
                theme::FG_SOFT
            } else {
                theme::FG_DIM
            },
        );
    }
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
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(RichText::new("Loading usage data...").size(13.0).color(theme::FG_SOFT));
    });
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
