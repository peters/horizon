use std::sync::Arc;

use egui::{Color32, CornerRadius, FontId, Galley, Id, Pos2, Rect, Stroke, Vec2};
use horizon_core::{AttentionSeverity, PanelId, SshConnectionStatus, flatten_and_truncate_chars};

use crate::badge::{BadgeStroke, paint_badge_background};
use crate::text::painter_text_galley;
use crate::theme;

use super::super::util::{format_compact_count, usize_to_f32};
use super::PanelChrome;

const ATTENTION_BADGE_HORIZONTAL_PADDING: f32 = 12.0;
const SSH_BADGE_HORIZONTAL_PADDING: f32 = 16.0;
const TITLEBAR_LEADING_CONTROL_SPACE: f32 = 60.0;
const STATUS_BADGE_GAP: f32 = 6.0;
const TITLE_BADGE_GAP: f32 = 4.0;
const STATUS_BADGE_FONT_SIZE: f32 = 10.0;

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
pub(in crate::app) struct StatusBadgeStrip {
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
    stroke: Option<BadgeStroke>,
}

struct StatusBadgeSpec<'a> {
    text: &'a str,
    compact_text: &'a str,
    compact_galley: Option<Arc<Galley>>,
    font: egui::FontId,
    color: Color32,
    fill: Color32,
    stroke: Option<BadgeStroke>,
    horizontal_padding: f32,
    right: f32,
    left_limit: f32,
    minimum_available_width: Option<f32>,
    center_text: bool,
}

/// Lays out the status strip once so painting, title elision, and inline
/// rename use the same measured badge edges.
#[profiling::function]
pub(in crate::app) fn layout_status_badges(painter: &egui::Painter, chrome: &PanelChrome<'_>) -> StatusBadgeStrip {
    let anchor = chrome.controls_anchor();
    let has_history_meter = chrome.scrollback_limit > 0;
    let trailing_right = trailing_status_badge_right(chrome.titlebar_rect, anchor, has_history_meter);
    let left_limit = chrome.titlebar_rect.min.x + TITLEBAR_LEADING_CONTROL_SPACE;
    let attention_compact_galley =
        chrome
            .attention_badge
            .filter(|_| chrome.ssh_status.is_some())
            .map(|(severity, _)| {
                let color = attention_severity_color(*severity);
                painter_text_galley(
                    painter,
                    attention_severity_icon(*severity),
                    &status_badge_font(),
                    color,
                    f32::INFINITY,
                )
            });
    let reserved_attention_width = attention_compact_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + ATTENTION_BADGE_HORIZONTAL_PADDING);
    let attention_reservation = if attention_compact_galley.is_some() {
        reserved_attention_width + STATUS_BADGE_GAP
    } else {
        0.0
    };

    let ssh = chrome.ssh_status.map(|status| {
        let color = ssh_status_color(status);
        layout_status_badge(
            painter,
            chrome.titlebar_rect,
            &StatusBadgeSpec {
                text: status.label(),
                compact_text: ssh_status_icon(status),
                compact_galley: None,
                font: status_badge_font(),
                color,
                fill: status_badge_fill(color, 72),
                stroke: Some(BadgeStroke::inside(Stroke::new(1.0_f32, theme::alpha(color, 140)))),
                horizontal_padding: SSH_BADGE_HORIZONTAL_PADDING,
                right: trailing_right,
                left_limit: left_limit + attention_reservation,
                minimum_available_width: None,
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
                compact_galley: attention_compact_galley.as_ref().map(Arc::clone),
                font: status_badge_font(),
                color,
                fill: status_badge_fill(color, 60),
                stroke: None,
                horizontal_padding: ATTENTION_BADGE_HORIZONTAL_PADDING,
                right: attention_right,
                left_limit,
                minimum_available_width: ssh.as_ref().map(|_| reserved_attention_width),
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

#[profiling::function]
fn layout_status_badge(painter: &egui::Painter, titlebar_rect: Rect, spec: &StatusBadgeSpec<'_>) -> StatusBadge {
    let available_width = (spec.right - spec.left_limit)
        .max(0.0)
        .max(spec.minimum_available_width.unwrap_or(0.0));
    let text_width = (available_width - spec.horizontal_padding).max(0.0);
    let bounded_galley = painter_text_galley(painter, spec.text, &spec.font, spec.color, text_width);
    let compact_galley = spec.compact_galley.as_ref().map_or_else(
        || painter_text_galley(painter, spec.compact_text, &spec.font, spec.color, f32::INFINITY),
        Arc::clone,
    );
    let use_compact = bounded_galley.elided && {
        let ellipsis_width = painter.fonts_mut(|fonts| fonts.glyph_width(&spec.font, '…'));
        text_width < compact_galley.size().x + ellipsis_width
    };
    let (galley, horizontal_padding) = if use_compact {
        if compact_galley.size().x + spec.horizontal_padding > available_width {
            let fallback_padding = 2.0;
            let fallback_galley = painter_text_galley(painter, "•", &spec.font, spec.color, f32::INFINITY);
            (fallback_galley, fallback_padding)
        } else {
            (compact_galley, spec.horizontal_padding)
        }
    } else {
        (bounded_galley, spec.horizontal_padding)
    };
    let badge_width = (galley.size().x + horizontal_padding).min(available_width);
    let badge_height = 18.0;
    let badge_left = (spec.right - badge_width).max(spec.left_limit);
    let rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new((spec.right - badge_left).max(0.0), badge_height),
    );
    let text_x = if spec.center_text {
        rect.center().x - galley.size().x * 0.5
    } else {
        rect.min.x + horizontal_padding * 0.5
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
        Some(BadgeStroke::outside(Stroke::new(
            1.0_f32,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE(), meter.accent, 0.34), 180),
        ))),
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
    let display_text = flatten_and_truncate_chars(summary, 30);
    format!("{icon} {display_text}")
}

fn status_badge_font() -> FontId {
    FontId::proportional(STATUS_BADGE_FONT_SIZE)
}

fn status_badge_fill(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, alpha)
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
    use egui::{Color32, Pos2, Rect, Vec2};
    use horizon_core::{AttentionSeverity, PanelId, PanelKind, SshConnectionStatus};

    use super::{
        StatusBadgeSpec, TITLE_BADGE_GAP, TITLEBAR_LEADING_CONTROL_SPACE, attention_badge_text, layout_status_badge,
        layout_status_badges,
    };
    use crate::app::panel_chrome::{MicControl, PanelChrome};
    use crate::app::speech::MicState;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StatusFeature {
        History,
        Ssh,
        Attention,
        Mic,
    }

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
    fn attention_badge_flattens_line_separators_before_truncating() {
        assert_eq!(
            attention_badge_text(AttentionSeverity::High, "Approve\r\ndeploy\u{2028}now"),
            "⚠ Approve deploy now"
        );
    }

    fn status_layout(width: f32, features: &[StatusFeature]) -> super::StatusBadgeStrip {
        status_layout_at_x_with_summary(10.0, width, features, "Claude needs your approval to run a command")
    }

    fn status_layout_with_summary(width: f32, features: &[StatusFeature], summary: &str) -> super::StatusBadgeStrip {
        status_layout_at_x_with_summary(10.0, width, features, summary)
    }

    fn status_layout_at_x_with_summary(
        x: f32,
        width: f32,
        features: &[StatusFeature],
        summary: &str,
    ) -> super::StatusBadgeStrip {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let attention_data = (AttentionSeverity::High, summary.to_string());
        let mut result = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let titlebar_rect = Rect::from_min_size(Pos2::new(x, 20.0), Vec2::new(width, 34.0));
                let close_rect = Rect::from_center_size(
                    Pos2::new(titlebar_rect.max.x - 18.0, titlebar_rect.center().y),
                    Vec2::splat(16.0),
                );
                let mic_rect = Rect::from_center_size(
                    Pos2::new(titlebar_rect.max.x - 44.0, titlebar_rect.center().y),
                    Vec2::splat(16.0),
                );
                let chrome = PanelChrome {
                    panel_id: PanelId(1),
                    kind: PanelKind::Shell,
                    panel_rect: Rect::from_min_size(Pos2::new(x, 20.0), Vec2::new(width, 340.0)),
                    titlebar_rect,
                    close_rect,
                    resize_rect: Rect::NOTHING,
                    title: Some("Panel title"),
                    history_size: 1,
                    scrollback_limit: usize::from(features.contains(&StatusFeature::History)),
                    focused: true,
                    close_hovered: false,
                    workspace_accent: None,
                    attention_badge: features.contains(&StatusFeature::Attention).then_some(&attention_data),
                    ssh_status: features
                        .contains(&StatusFeature::Ssh)
                        .then_some(SshConnectionStatus::Disconnected),
                    mic: features.contains(&StatusFeature::Mic).then_some(MicControl {
                        rect: mic_rect,
                        hovered: false,
                        state: MicState::Idle,
                    }),
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
        let layout = status_layout(1_200.0, &[StatusFeature::Attention]);
        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };

        assert!(!attention.galley.elided);
        assert_eq!(attention.galley.job.text, "⚠ Claude needs your approval to…");
        assert!(attention.rect.width() > 110.0);
    }

    #[test]
    fn minimum_width_strip_keeps_compact_attention_and_elided_ssh_visible() {
        let layout = status_layout(
            320.0,
            &[
                StatusFeature::History,
                StatusFeature::Ssh,
                StatusFeature::Attention,
                StatusFeature::Mic,
            ],
        );
        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };
        let Some(ssh) = layout.ssh else {
            panic!("SSH badge was not laid out");
        };

        assert!(!attention.galley.elided);
        assert_eq!(attention.galley.job.text, "⚠");
        assert!(ssh.galley.elided);
        assert_eq!(ssh.galley.job.text, "Disconnected");
        assert!(attention.rect.min.x >= 10.0 + TITLEBAR_LEADING_CONTROL_SPACE);
        assert!(attention.rect.max.x < ssh.rect.min.x);
        assert_near(layout.title_right, attention.rect.min.x - TITLE_BADGE_GAP);
    }

    #[test]
    fn compact_attention_reservation_survives_fractional_panel_positions() {
        let layout = status_layout_at_x_with_summary(
            169.7433,
            283.7605,
            &[
                StatusFeature::History,
                StatusFeature::Ssh,
                StatusFeature::Attention,
                StatusFeature::Mic,
            ],
            "Claude needs your approval to run a command",
        );

        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };
        assert_eq!(attention.galley.job.text, "⚠");
    }

    #[test]
    fn moderate_strip_elides_attention_summary_before_compacting_to_icon() {
        let summary = "二".repeat(30);
        let layout = status_layout_with_summary(
            440.0,
            &[StatusFeature::History, StatusFeature::Ssh, StatusFeature::Attention],
            &summary,
        );
        let Some(attention) = layout.attention else {
            panic!("attention badge was not laid out");
        };

        assert!(attention.galley.elided);
        assert!(attention.galley.job.text.starts_with("⚠ 二"));
    }

    #[test]
    fn compact_badge_degrades_to_a_bare_status_dot() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::configure_fonts());
        let mut result = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                result = Some(layout_status_badge(
                    ui.painter(),
                    Rect::from_min_size(Pos2::ZERO, Vec2::new(80.0, 34.0)),
                    &StatusBadgeSpec {
                        text: "Disconnected",
                        compact_text: "×",
                        compact_galley: None,
                        font: egui::FontId::proportional(10.0),
                        color: Color32::WHITE,
                        fill: Color32::BLACK,
                        stroke: None,
                        horizontal_padding: 16.0,
                        right: 55.0,
                        left_limit: 45.0,
                        minimum_available_width: None,
                        center_text: true,
                    },
                ));
            });
        });

        let Some(badge) = result else {
            panic!("status dot was not laid out");
        };
        assert_eq!(badge.galley.job.text, "•");
    }

    #[test]
    fn title_boundary_tracks_the_actual_leftmost_painted_badge() {
        let ssh_only = status_layout(800.0, &[StatusFeature::Ssh]);
        let Some(ssh) = ssh_only.ssh else {
            panic!("SSH badge was not laid out");
        };
        assert_near(ssh_only.title_right, ssh.rect.min.x - TITLE_BADGE_GAP);

        let both = status_layout(
            800.0,
            &[StatusFeature::History, StatusFeature::Ssh, StatusFeature::Attention],
        );
        let Some(attention) = both.attention else {
            panic!("attention badge was not laid out");
        };
        assert_near(both.title_right, attention.rect.min.x - TITLE_BADGE_GAP);
    }
}
