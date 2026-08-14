use egui::{Color32, CornerRadius, Id, Pos2, Rect, Stroke, StrokeKind, Vec2};
use horizon_core::{AttentionSeverity, PanelId, SshConnectionStatus, truncate_chars};

use crate::badge::paint_badge_background;
use crate::text::single_line_label_job;
use crate::theme;

use super::super::util::{format_compact_count, usize_to_f32};
use super::PanelChrome;

const ATTENTION_BADGE_HORIZONTAL_PADDING: f32 = 12.0;
const ATTENTION_BADGE_RESERVED_WIDTH: f32 = 110.0;
const SSH_BADGE_HORIZONTAL_PADDING: f32 = 16.0;
const SSH_BADGE_RESERVED_WIDTH: f32 = 90.0;
const TITLEBAR_LEADING_CONTROL_SPACE: f32 = 60.0;

#[derive(Clone, Copy)]
pub(super) struct HistoryMeter {
    pub(super) panel_id: PanelId,
    pub(super) titlebar_rect: Rect,
    pub(super) close_rect: Rect,
    pub(super) accent: Color32,
    pub(super) history_size: usize,
    pub(super) scrollback_limit: usize,
    pub(super) focused: bool,
}

/// Compute the right x boundary where the title text must stop, accounting
/// for all badges (history meter, SSH status, attention) that sit to its right.
pub(super) fn title_right_boundary(chrome: &PanelChrome<'_>) -> f32 {
    let anchor = chrome.controls_anchor();
    let has_history_meter = chrome.scrollback_limit > 0;
    let title_gap = if has_history_meter { 2.0 } else { 4.0 };
    let mut right = trailing_status_badge_right(chrome.titlebar_rect, anchor, has_history_meter) - title_gap;
    if chrome.ssh_status.is_some() {
        right -= SSH_BADGE_RESERVED_WIDTH;
    }
    if chrome.attention_badge.is_some() {
        right -= ATTENTION_BADGE_RESERVED_WIDTH;
    }
    right
}

#[profiling::function]
pub(super) fn paint_history_meter(ui: &egui::Ui, painter: &egui::Painter, meter: HistoryMeter) {
    let badge_rect = panel_history_badge_rect(meter.titlebar_rect, meter.close_rect);
    let track_rect = Rect::from_min_max(
        Pos2::new(badge_rect.min.x + 8.0, badge_rect.max.y - 5.0),
        Pos2::new(badge_rect.max.x - 8.0, badge_rect.max.y - 3.0),
    );
    let ratio = if meter.scrollback_limit == 0 {
        0.0
    } else {
        (usize_to_f32(meter.history_size) / usize_to_f32(meter.scrollback_limit)).clamp(0.0, 1.0)
    };
    let animated_ratio =
        ui.ctx()
            .animate_value_with_time(Id::new(("panel_history_ratio", meter.panel_id.0)), ratio, 0.16);
    let fill_width = track_rect.width() * animated_ratio.clamp(0.0, 1.0);
    let fill_rect = Rect::from_min_max(
        track_rect.min,
        Pos2::new(track_rect.min.x + fill_width, track_rect.max.y),
    );
    let history_text = format!(
        "{}/{}",
        format_compact_count(meter.history_size),
        format_compact_count(meter.scrollback_limit)
    );

    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(7),
        theme::alpha(
            theme::blend(theme::BG_ELEVATED(), meter.accent, 0.10),
            if meter.focused { 214 } else { 184 },
        ),
        Some((
            Stroke::new(
                1.0_f32,
                theme::alpha(theme::blend(theme::BORDER_SUBTLE(), meter.accent, 0.34), 180),
            ),
            StrokeKind::Outside,
        )),
    );
    painter.rect_filled(track_rect, CornerRadius::same(2), theme::alpha(theme::FG_DIM(), 52));
    if fill_width > 0.0 {
        painter.rect_filled(
            fill_rect,
            CornerRadius::same(2),
            theme::alpha(
                theme::blend(theme::ACCENT(), meter.accent, 0.35),
                if meter.focused { 224 } else { 188 },
            ),
        );
    }
    painter.text(
        Pos2::new(badge_rect.center().x, badge_rect.center().y - 2.0),
        egui::Align2::CENTER_CENTER,
        history_text,
        egui::FontId::monospace(10.5),
        if meter.history_size > 0 {
            theme::FG_SOFT()
        } else {
            theme::FG_DIM()
        },
    );
}

#[profiling::function]
pub(super) fn paint_attention_badge(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    has_history_meter: bool,
    has_ssh_badge: bool,
    severity: AttentionSeverity,
    summary: &str,
) {
    let color = attention_severity_color(severity);
    let badge_right = attention_badge_right(titlebar_rect, close_rect, has_history_meter, has_ssh_badge);
    let badge_left_limit = titlebar_rect.min.x + TITLEBAR_LEADING_CONTROL_SPACE;
    let Some(max_badge_width) = badge_width_budget(
        badge_right - badge_left_limit,
        ATTENTION_BADGE_RESERVED_WIDTH,
        ATTENTION_BADGE_HORIZONTAL_PADDING,
    ) else {
        return;
    };
    let max_text_width = max_badge_width - ATTENTION_BADGE_HORIZONTAL_PADDING;
    let text_galley = painter.layout_job(attention_badge_layout_job(severity, summary, color, max_text_width));
    let badge_width = (text_galley.size().x + ATTENTION_BADGE_HORIZONTAL_PADDING).min(max_badge_width);
    let badge_height: f32 = 18.0;
    let badge_left = badge_right - badge_width;
    let badge_rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_width, badge_height),
    );

    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
        None,
    );
    painter.with_clip_rect(badge_rect).galley(
        Pos2::new(
            badge_left + ATTENTION_BADGE_HORIZONTAL_PADDING * 0.5,
            titlebar_rect.center().y - text_galley.size().y * 0.5,
        ),
        text_galley,
        Color32::TRANSPARENT,
    );
}

fn attention_badge_text(severity: AttentionSeverity, summary: &str) -> String {
    let icon = attention_severity_icon(severity);
    let display_text = truncate_chars(summary, 30);
    format!("{icon} {display_text}")
}

fn attention_badge_layout_job(
    severity: AttentionSeverity,
    summary: &str,
    color: Color32,
    max_width: f32,
) -> egui::text::LayoutJob {
    single_line_label_job(
        &attention_badge_text(severity, summary),
        &egui::FontId::proportional(10.0),
        color,
        max_width,
    )
}

#[profiling::function]
pub(super) fn paint_ssh_status_badge(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    has_history_meter: bool,
    status: SshConnectionStatus,
) {
    let color = ssh_status_color(status);
    let badge_right = trailing_status_badge_right(titlebar_rect, close_rect, has_history_meter);
    let badge_left_limit = titlebar_rect.min.x + TITLEBAR_LEADING_CONTROL_SPACE;
    let Some(max_badge_width) = badge_width_budget(
        badge_right - badge_left_limit,
        SSH_BADGE_RESERVED_WIDTH,
        SSH_BADGE_HORIZONTAL_PADDING,
    ) else {
        return;
    };
    let text_galley = painter.layout_job(ssh_status_badge_layout_job(
        status,
        color,
        max_badge_width - SSH_BADGE_HORIZONTAL_PADDING,
    ));
    let badge_width = (text_galley.size().x + SSH_BADGE_HORIZONTAL_PADDING).min(max_badge_width);
    let badge_height = 18.0;
    let badge_left = badge_right - badge_width;
    let badge_rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_width, badge_height),
    );

    paint_badge_background(
        painter,
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 72),
        Some((Stroke::new(1.0_f32, theme::alpha(color, 140)), StrokeKind::Inside)),
    );
    painter.with_clip_rect(badge_rect).galley(
        Pos2::new(
            badge_rect.center().x - text_galley.size().x * 0.5,
            badge_rect.center().y - text_galley.size().y * 0.5,
        ),
        text_galley,
        Color32::TRANSPARENT,
    );
}

fn ssh_status_badge_layout_job(status: SshConnectionStatus, color: Color32, max_width: f32) -> egui::text::LayoutJob {
    single_line_label_job(status.label(), &egui::FontId::proportional(10.0), color, max_width)
}

fn badge_width_budget(available_width: f32, reserved_width: f32, horizontal_padding: f32) -> Option<f32> {
    let width = available_width.clamp(0.0, reserved_width);
    (width > horizontal_padding).then_some(width)
}

fn ssh_status_color(status: SshConnectionStatus) -> Color32 {
    match status {
        SshConnectionStatus::Connecting => theme::PALETTE_YELLOW(),
        SshConnectionStatus::Connected => theme::PALETTE_GREEN(),
        SshConnectionStatus::Disconnected => theme::PALETTE_RED(),
    }
}

fn attention_severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED(),
        AttentionSeverity::Medium => theme::PALETTE_GREEN(),
        AttentionSeverity::Low => theme::ACCENT(),
    }
}

fn attention_severity_icon(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "\u{26A0}",
        AttentionSeverity::Medium => "\u{2713}",
        AttentionSeverity::Low => "\u{2139}",
    }
}

pub(super) fn panel_history_badge_rect(titlebar_rect: Rect, close_rect: Rect) -> Rect {
    let badge_size = Vec2::new(96.0, 20.0);
    Rect::from_center_size(
        Pos2::new(close_rect.min.x - (badge_size.x * 0.5) - 10.0, titlebar_rect.center().y),
        badge_size,
    )
}

fn trailing_status_badge_right(titlebar_rect: Rect, close_rect: Rect, has_history_meter: bool) -> f32 {
    if has_history_meter {
        panel_history_badge_rect(titlebar_rect, close_rect).min.x - 6.0
    } else {
        close_rect.min.x - 8.0
    }
}

fn attention_badge_right(titlebar_rect: Rect, close_rect: Rect, has_history_meter: bool, has_ssh_badge: bool) -> f32 {
    trailing_status_badge_right(titlebar_rect, close_rect, has_history_meter)
        - if has_ssh_badge { SSH_BADGE_RESERVED_WIDTH } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Pos2, Rect};
    use horizon_core::{AttentionSeverity, SshConnectionStatus};

    use super::{
        ATTENTION_BADGE_HORIZONTAL_PADDING, SSH_BADGE_HORIZONTAL_PADDING, SSH_BADGE_RESERVED_WIDTH,
        attention_badge_layout_job, attention_badge_right, attention_badge_text, badge_width_budget,
        panel_history_badge_rect, ssh_status_badge_layout_job,
    };

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() <= 0.001, "expected {expected}, got {actual}");
    }

    #[test]
    fn attention_badge_truncates_utf8_summary_without_slicing_bytes() {
        let summary = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaåzz";

        assert_eq!(
            attention_badge_text(AttentionSeverity::Low, summary),
            "ℹ aaaaaaaaaaaaaaaaaaaaaaaaaaaaå…"
        );
    }

    #[test]
    fn attention_badge_layout_is_single_line_and_pixel_bounded() {
        let job = attention_badge_layout_job(
            AttentionSeverity::Low,
            "first\r\nsecond 二二二二二二二二二二二二二二二二二二二二二二二二二二二二二二",
            Color32::WHITE,
            98.0,
        );

        assert!(!job.text.contains(['\r', '\n']));
        assert!(!job.break_on_newline);
        assert_near(job.wrap.max_width, 98.0);
        assert_eq!(job.wrap.max_rows, 1);
        assert_eq!(job.wrap.overflow_character, Some('…'));
    }

    #[test]
    fn badge_budget_rejects_empty_chips_at_or_below_padding() {
        assert_eq!(
            badge_width_budget(-1.0, 110.0, ATTENTION_BADGE_HORIZONTAL_PADDING),
            None
        );
        assert_eq!(
            badge_width_budget(ATTENTION_BADGE_HORIZONTAL_PADDING, 110.0, 12.0),
            None
        );
        assert_eq!(badge_width_budget(12.5, 110.0, 12.0), Some(12.5));
        assert_eq!(badge_width_budget(140.0, 110.0, 12.0), Some(110.0));
    }

    #[test]
    fn attention_badge_uses_the_visible_trailing_anchor_and_stacks_left_of_ssh() {
        let titlebar_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(410.0, 54.0));
        let controls_rect = Rect::from_min_max(Pos2::new(370.0, 20.0), Pos2::new(390.0, 54.0));

        let without_history = attention_badge_right(titlebar_rect, controls_rect, false, false);
        assert_near(without_history, controls_rect.min.x - 8.0);

        let with_history = attention_badge_right(titlebar_rect, controls_rect, true, false);
        assert_near(
            with_history,
            panel_history_badge_rect(titlebar_rect, controls_rect).min.x - 6.0,
        );

        let with_ssh = attention_badge_right(titlebar_rect, controls_rect, false, true);
        assert_near(with_ssh, without_history - SSH_BADGE_RESERVED_WIDTH);
    }

    #[test]
    fn all_ssh_status_badges_are_single_line_and_reserved_width_bounded() {
        for status in [
            SshConnectionStatus::Connecting,
            SshConnectionStatus::Connected,
            SshConnectionStatus::Disconnected,
        ] {
            let job = ssh_status_badge_layout_job(
                status,
                Color32::WHITE,
                SSH_BADGE_RESERVED_WIDTH - SSH_BADGE_HORIZONTAL_PADDING,
            );

            assert!(!job.break_on_newline);
            assert_eq!(job.wrap.max_rows, 1);
            assert_near(
                job.wrap.max_width,
                SSH_BADGE_RESERVED_WIDTH - SSH_BADGE_HORIZONTAL_PADDING,
            );
            assert_eq!(job.wrap.overflow_character, Some('…'));
        }
    }
}
