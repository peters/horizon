use std::sync::Arc;

use egui::{Color32, CornerRadius, Galley, Id, Pos2, Rect, Stroke, StrokeKind, Vec2};
use horizon_core::{AttentionSeverity, PanelId, SshConnectionStatus, truncate_chars};

use crate::badge::paint_badge_background;
use crate::text::single_line_label_job;
use crate::theme;

use super::super::util::{format_compact_count, usize_to_f32};
use super::PanelChrome;

const ATTENTION_BADGE_HORIZONTAL_PADDING: f32 = 12.0;
const SSH_BADGE_HORIZONTAL_PADDING: f32 = 16.0;
const TITLEBAR_LEADING_CONTROL_SPACE: f32 = 60.0;
const STATUS_BADGE_GAP: f32 = 6.0;
const TITLE_BADGE_GAP: f32 = 4.0;

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

#[derive(Clone)]
pub(super) struct StatusBadgeStrip {
    pub(super) title_right: f32,
    attention: Option<StatusBadge>,
    ssh: Option<StatusBadge>,
}

#[derive(Clone)]
struct StatusBadge {
    rect: Rect,
    galley: Arc<Galley>,
    text_position: Pos2,
    fill: Color32,
    stroke: Option<(Stroke, StrokeKind)>,
}

struct StatusBadgeSpec<'a> {
    text: &'a str,
    compact_text: &'a str,
    font: egui::FontId,
    color: Color32,
    fill: Color32,
    stroke: Option<(Stroke, StrokeKind)>,
    horizontal_padding: f32,
    right: f32,
    left_limit: f32,
    center_text: bool,
}

/// Lays out the status strip once so painting, title elision, and inline
/// rename use the same measured badge edges.
pub(super) fn layout_status_badges(painter: &egui::Painter, chrome: &PanelChrome<'_>) -> StatusBadgeStrip {
    let anchor = chrome.controls_anchor();
    let has_history_meter = chrome.scrollback_limit > 0;
    let trailing_right = trailing_status_badge_right(chrome.titlebar_rect, anchor, has_history_meter);
    let left_limit = chrome.titlebar_rect.min.x + TITLEBAR_LEADING_CONTROL_SPACE;

    let ssh = chrome.ssh_status.map(|status| {
        let color = ssh_status_color(status);
        layout_status_badge(
            painter,
            chrome.titlebar_rect,
            &StatusBadgeSpec {
                text: status.label(),
                compact_text: ssh_status_icon(status),
                font: egui::FontId::proportional(10.0),
                color,
                fill: Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 72),
                stroke: Some((Stroke::new(1.0_f32, theme::alpha(color, 140)), StrokeKind::Inside)),
                horizontal_padding: SSH_BADGE_HORIZONTAL_PADDING,
                right: trailing_right,
                left_limit,
                center_text: true,
            },
        )
    });

    let attention_right = ssh
        .as_ref()
        .map_or(trailing_right, |badge| badge.rect.min.x - STATUS_BADGE_GAP);
    let attention = chrome.attention_badge.map(|(severity, summary)| {
        let color = attention_severity_color(*severity);
        let text = attention_badge_text(*severity, summary);
        layout_status_badge(
            painter,
            chrome.titlebar_rect,
            &StatusBadgeSpec {
                text: &text,
                compact_text: attention_severity_icon(*severity),
                font: egui::FontId::proportional(10.0),
                color,
                fill: Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
                stroke: None,
                horizontal_padding: ATTENTION_BADGE_HORIZONTAL_PADDING,
                right: attention_right,
                left_limit,
                center_text: false,
            },
        )
    });

    let leftmost_badge = attention.as_ref().or(ssh.as_ref());
    let title_right = leftmost_badge.map_or(trailing_right - TITLE_BADGE_GAP, |badge| {
        badge.rect.min.x - TITLE_BADGE_GAP
    });

    StatusBadgeStrip {
        title_right,
        attention,
        ssh,
    }
}

fn layout_status_badge(painter: &egui::Painter, titlebar_rect: Rect, spec: &StatusBadgeSpec<'_>) -> StatusBadge {
    let natural_galley = painter.layout_job(single_line_label_job(spec.text, &spec.font, spec.color, f32::INFINITY));
    let compact_galley = painter.layout_job(single_line_label_job(
        spec.compact_text,
        &spec.font,
        spec.color,
        f32::INFINITY,
    ));
    let minimum_width = compact_galley.size().x + spec.horizontal_padding;
    let available_width = (spec.right - spec.left_limit).max(0.0);
    let natural_width = natural_galley.size().x + spec.horizontal_padding;
    let badge_width = natural_width.min(available_width).max(minimum_width);
    let use_compact_text = badge_width <= minimum_width + f32::EPSILON && natural_width > minimum_width;
    let text = if use_compact_text { spec.compact_text } else { spec.text };
    let text_width = (badge_width - spec.horizontal_padding).max(compact_galley.size().x);
    let galley = painter.layout_job(single_line_label_job(text, &spec.font, spec.color, text_width));
    let badge_height = 18.0;
    let rect = Rect::from_min_size(
        Pos2::new(spec.right - badge_width, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_width, badge_height),
    );
    let text_x = if spec.center_text {
        rect.center().x - galley.size().x * 0.5
    } else {
        rect.min.x + spec.horizontal_padding * 0.5
    };
    let text_position = Pos2::new(text_x, rect.center().y - galley.size().y * 0.5);

    StatusBadge {
        rect,
        galley,
        text_position,
        fill: spec.fill,
        stroke: spec.stroke,
    }
}

#[profiling::function]
pub(super) fn paint_status_badges(painter: &egui::Painter, strip: &StatusBadgeStrip) {
    if let Some(attention) = &strip.attention {
        paint_status_badge(painter, attention);
    }
    if let Some(ssh) = &strip.ssh {
        paint_status_badge(painter, ssh);
    }
}

fn paint_status_badge(painter: &egui::Painter, badge: &StatusBadge) {
    paint_badge_background(painter, badge.rect, CornerRadius::same(4), badge.fill, badge.stroke);
    painter
        .with_clip_rect(badge.rect)
        .galley(badge.text_position, Arc::clone(&badge.galley), Color32::TRANSPARENT);
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

fn attention_badge_text(severity: AttentionSeverity, summary: &str) -> String {
    let icon = attention_severity_icon(severity);
    let display_text = truncate_chars(summary, 30);
    format!("{icon} {display_text}")
}

#[cfg(test)]
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

#[cfg(test)]
fn ssh_status_badge_layout_job(status: SshConnectionStatus, color: Color32, max_width: f32) -> egui::text::LayoutJob {
    single_line_label_job(status.label(), &egui::FontId::proportional(10.0), color, max_width)
}

fn ssh_status_icon(status: SshConnectionStatus) -> &'static str {
    match status {
        SshConnectionStatus::Connecting => "◌",
        SshConnectionStatus::Connected => "●",
        SshConnectionStatus::Disconnected => "×",
    }
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use egui::{Color32, Pos2, Rect, Vec2};
    use horizon_core::{AttentionSeverity, PanelId, PanelKind, SshConnectionStatus};

    use super::{
        ATTENTION_BADGE_HORIZONTAL_PADDING, SSH_BADGE_HORIZONTAL_PADDING, TITLE_BADGE_GAP, attention_badge_layout_job,
        attention_badge_text, layout_status_badges, ssh_status_badge_layout_job,
    };
    use crate::app::panel_chrome::PanelChrome;

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

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let width = Cell::new(0.0_f32);
        let elided = Cell::new(false);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let galley = ui.painter().layout_job(job.clone());
                width.set(galley.size().x);
                elided.set(galley.elided);
            });
        });
        assert!(width.get() <= 98.0);
        assert!(elided.get());
    }

    fn status_layout(width: f32, history: bool, ssh: bool, attention: bool) -> super::StatusBadgeStrip {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let attention_data = (
            AttentionSeverity::High,
            "Claude needs your approval to run a command".to_string(),
        );
        let mut result = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let titlebar_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(width, 34.0));
                let close_rect = Rect::from_center_size(
                    Pos2::new(titlebar_rect.max.x - 20.0, titlebar_rect.center().y),
                    Vec2::splat(20.0),
                );
                let chrome = PanelChrome {
                    panel_id: PanelId(1),
                    kind: PanelKind::Shell,
                    panel_rect: Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(width, 340.0)),
                    titlebar_rect,
                    close_rect,
                    resize_rect: Rect::NOTHING,
                    title: Some("Panel title"),
                    history_size: 1,
                    scrollback_limit: usize::from(history),
                    focused: true,
                    close_hovered: false,
                    workspace_accent: None,
                    attention_badge: attention.then_some(&attention_data),
                    ssh_status: ssh.then_some(SshConnectionStatus::Disconnected),
                    mic: None,
                };
                result = Some(layout_status_badges(ui.painter(), &chrome));
            });
        });
        let Some(result) = result else {
            panic!("badge layout was not produced");
        };
        result
    }

    #[test]
    fn wide_attention_badge_keeps_the_complete_thirty_character_summary() {
        let layout = status_layout(1_200.0, false, false, true);
        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };

        assert!(!attention.galley.elided);
        assert_eq!(
            attention.galley.job.text,
            attention_badge_text(AttentionSeverity::High, "Claude needs your approval to run a command")
        );
        assert!(attention.rect.width() > 110.0);
    }

    #[test]
    fn narrow_strip_keeps_compact_attention_and_ssh_signals_visible() {
        let layout = status_layout(220.0, true, true, true);
        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };
        let Some(ssh) = layout.ssh else {
            panic!("SSH badge was not laid out");
        };

        assert!(attention.rect.width() > ATTENTION_BADGE_HORIZONTAL_PADDING);
        assert!(ssh.rect.width() > SSH_BADGE_HORIZONTAL_PADDING);
        assert!(attention.rect.max.x < ssh.rect.min.x);
        assert_near(layout.title_right, attention.rect.min.x - TITLE_BADGE_GAP);
    }

    #[test]
    fn title_boundary_tracks_the_actual_leftmost_painted_badge() {
        let ssh_only = status_layout(800.0, false, true, false);
        let Some(ssh) = ssh_only.ssh else {
            panic!("SSH badge was not laid out");
        };
        assert_near(ssh_only.title_right, ssh.rect.min.x - TITLE_BADGE_GAP);

        let both = status_layout(800.0, true, true, true);
        let Some(attention) = both.attention else {
            panic!("attention badge was not laid out");
        };
        assert_near(both.title_right, attention.rect.min.x - TITLE_BADGE_GAP);
    }

    #[test]
    fn all_ssh_status_badges_are_single_line_and_pixel_bounded() {
        for status in [
            SshConnectionStatus::Connecting,
            SshConnectionStatus::Connected,
            SshConnectionStatus::Disconnected,
        ] {
            let job = ssh_status_badge_layout_job(status, Color32::WHITE, 74.0);

            assert!(!job.break_on_newline);
            assert_eq!(job.wrap.max_rows, 1);
            assert_near(job.wrap.max_width, 74.0);
            assert_eq!(job.wrap.overflow_character, Some('…'));
        }
    }
}
