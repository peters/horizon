use egui::{Align, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect, Stroke, StrokeKind, UiBuilder, Vec2};
use horizon_core::{AttentionSeverity, PanelId, PanelKind, SshConnectionStatus, agent_definition};

use crate::theme;

use super::RenameEditAction;
use super::util::{format_compact_count, usize_to_f32};

#[derive(Clone, Copy)]
pub(super) struct PanelChrome<'a> {
    pub panel_id: PanelId,
    pub kind: PanelKind,
    pub panel_rect: Rect,
    pub titlebar_rect: Rect,
    pub close_rect: Rect,
    pub resize_rect: Rect,
    pub title: Option<&'a str>,
    pub history_size: usize,
    pub scrollback_limit: usize,
    pub focused: bool,
    pub close_hovered: bool,
    pub workspace_accent: Option<Color32>,
    pub attention_badge: Option<&'a (AttentionSeverity, String)>,
    pub ssh_status: Option<SshConnectionStatus>,
}

#[derive(Clone, Copy)]
struct HistoryMeter {
    panel_id: PanelId,
    titlebar_rect: Rect,
    close_rect: Rect,
    accent: Color32,
    history_size: usize,
    scrollback_limit: usize,
    focused: bool,
}

fn panel_accent(workspace_accent: Option<Color32>, focused: bool) -> Color32 {
    workspace_accent.unwrap_or(if focused { theme::ACCENT } else { theme::BORDER_STRONG })
}

fn panel_fill(accent: Color32, focused: bool) -> Color32 {
    theme::blend(theme::PANEL_BG, accent, if focused { 0.06 } else { 0.0 })
}

fn panel_border_stroke(accent: Color32, focused: bool) -> Stroke {
    Stroke::new(if focused { 1.8 } else { 1.2 }, theme::panel_border(accent, focused))
}

fn panel_titlebar_fill(accent: Color32, focused: bool) -> Color32 {
    theme::blend(theme::PANEL_BG_ALT, accent, if focused { 0.28 } else { 0.10 })
}

fn panel_title_color(focused: bool) -> Color32 {
    if focused { theme::FG } else { theme::FG_SOFT }
}

fn focus_ring_stroke(accent: Color32, focused: bool) -> Option<Stroke> {
    focused.then(|| Stroke::new(3.0, theme::alpha(theme::blend(theme::ACCENT, accent, 0.35), 56)))
}

fn title_focus_indicator_rect(titlebar_rect: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(titlebar_rect.min.x + 12.0, titlebar_rect.max.y - 4.0),
        Vec2::new(44.0, 2.5),
    )
}

pub(super) fn panel_kind_icon(kind: PanelKind, workspace_color: Color32, focused: bool) -> (&'static str, Color32) {
    if let Some(definition) = agent_definition(kind) {
        let [r, g, b] = definition.accent_rgb;
        return (
            definition.icon_label,
            theme::alpha(Color32::from_rgb(r, g, b), if focused { 220 } else { 120 }),
        );
    }

    match kind {
        PanelKind::Shell | PanelKind::Command => (">_", theme::alpha(workspace_color, if focused { 200 } else { 80 })),
        PanelKind::Ssh => (
            "SSH",
            theme::alpha(Color32::from_rgb(250, 179, 135), if focused { 220 } else { 150 }),
        ),
        PanelKind::Editor => (
            "MD",
            theme::alpha(Color32::from_rgb(166, 227, 161), if focused { 220 } else { 120 }),
        ),
        PanelKind::GitChanges => (
            "GC",
            theme::alpha(Color32::from_rgb(249, 226, 175), if focused { 220 } else { 120 }),
        ),
        PanelKind::Usage => (
            "US",
            theme::alpha(Color32::from_rgb(233, 190, 109), if focused { 220 } else { 120 }),
        ),
        PanelKind::Codex | PanelKind::Claude | PanelKind::OpenCode | PanelKind::Gemini | PanelKind::KiloCode => {
            unreachable!()
        }
    }
}

#[profiling::function]
pub(super) fn paint_panel_chrome(ui: &mut egui::Ui, chrome: PanelChrome<'_>) {
    let painter = ui.painter_at(chrome.panel_rect);
    let accent = panel_chrome_accent(chrome.kind, chrome.workspace_accent, chrome.focused);

    if let Some(stroke) = focus_ring_stroke(accent, chrome.focused) {
        painter.rect_stroke(
            chrome.panel_rect.expand(2.0),
            CornerRadius::same(18),
            stroke,
            StrokeKind::Outside,
        );
    }

    painter.rect_filled(
        chrome.panel_rect,
        CornerRadius::same(16),
        panel_fill(accent, chrome.focused),
    );
    painter.rect_stroke(
        chrome.panel_rect,
        CornerRadius::same(16),
        panel_border_stroke(accent, chrome.focused),
        StrokeKind::Outside,
    );
    painter.rect_filled(
        chrome.titlebar_rect,
        CornerRadius::same(16),
        panel_titlebar_fill(accent, chrome.focused),
    );
    if chrome.focused {
        painter.rect_filled(
            title_focus_indicator_rect(chrome.titlebar_rect),
            CornerRadius::same(2),
            theme::alpha(accent, 220),
        );
    }

    if let Some(title) = chrome.title {
        let title_x = if let Some(color) = chrome.workspace_accent {
            painter.circle_filled(
                Pos2::new(chrome.titlebar_rect.min.x + 14.0, chrome.titlebar_rect.center().y),
                if chrome.focused { 5.0 } else { 4.5 },
                theme::alpha(color, if chrome.focused { 240 } else { 180 }),
            );
            chrome.titlebar_rect.min.x + 26.0
        } else {
            chrome.titlebar_rect.min.x + 12.0
        };
        let title_right = title_right_boundary(&chrome);
        let max_width = (title_right - title_x).max(0.0);
        paint_truncated_title(
            &painter,
            title,
            title_x,
            chrome.titlebar_rect.center().y,
            max_width,
            chrome.focused,
        );
    }

    if let Some((severity, summary)) = chrome.attention_badge {
        paint_attention_badge(&painter, chrome.titlebar_rect, chrome.close_rect, *severity, summary);
    }
    if let Some(status) = chrome.ssh_status {
        paint_ssh_status_badge(
            &painter,
            chrome.titlebar_rect,
            chrome.close_rect,
            chrome.scrollback_limit > 0,
            status,
        );
    }

    if chrome.scrollback_limit > 0 {
        paint_history_meter(
            ui,
            &painter,
            HistoryMeter {
                panel_id: chrome.panel_id,
                titlebar_rect: chrome.titlebar_rect,
                close_rect: chrome.close_rect,
                accent,
                history_size: chrome.history_size,
                scrollback_limit: chrome.scrollback_limit,
                focused: chrome.focused,
            },
        );
    }

    paint_close_and_resize_controls(&painter, chrome.close_rect, chrome.resize_rect, chrome.close_hovered);
}

fn paint_close_and_resize_controls(painter: &egui::Painter, close_rect: Rect, resize_rect: Rect, close_hovered: bool) {
    painter.circle_filled(
        close_rect.center(),
        5.0,
        if close_hovered {
            theme::BTN_CLOSE
        } else {
            theme::alpha(theme::FG_DIM, 140)
        },
    );

    let handle_stroke = Stroke::new(1.0, theme::alpha(theme::FG_DIM, 170));
    painter.line_segment(
        [
            resize_rect.right_bottom(),
            resize_rect.left_top() + Vec2::new(6.0, 12.0),
        ],
        handle_stroke,
    );
    painter.line_segment(
        [
            resize_rect.right_bottom() - Vec2::new(0.0, 6.0),
            resize_rect.left_top() + Vec2::new(12.0, 12.0),
        ],
        handle_stroke,
    );
}

/// Compute the right x boundary where the title text must stop, accounting
/// for all badges (history meter, SSH status, attention) that sit to its right.
fn title_right_boundary(chrome: &PanelChrome<'_>) -> f32 {
    let mut right = chrome.close_rect.min.x - 12.0;
    if chrome.scrollback_limit > 0 {
        right = panel_history_badge_rect(chrome.titlebar_rect, chrome.close_rect).min.x - 8.0;
    }
    if chrome.ssh_status.is_some() {
        // SSH badge sits left of the history meter; reserve ~90px.
        right -= 90.0;
    }
    if chrome.attention_badge.is_some() {
        // Attention badge sits left of the history meter; reserve ~110px.
        right -= 110.0;
    }
    right
}

#[profiling::function]
fn paint_truncated_title(painter: &egui::Painter, title: &str, x: f32, center_y: f32, max_width: f32, focused: bool) {
    use egui::text::{LayoutJob, TextFormat, TextWrapping};

    let mut job = LayoutJob::single_section(
        title.to_string(),
        TextFormat {
            font_id: egui::FontId::proportional(13.0),
            color: panel_title_color(focused),
            ..Default::default()
        },
    );
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    let galley = painter.layout_job(job);
    let text_height = galley.size().y;
    painter.galley(Pos2::new(x, center_y - text_height * 0.5), galley, Color32::TRANSPARENT);
}

fn panel_chrome_accent(kind: PanelKind, workspace_accent: Option<Color32>, focused: bool) -> Color32 {
    if kind == PanelKind::Ssh {
        return theme::alpha(Color32::from_rgb(250, 179, 135), if focused { 220 } else { 170 });
    }
    panel_accent(workspace_accent, focused)
}

#[profiling::function]
fn paint_history_meter(ui: &egui::Ui, painter: &egui::Painter, meter: HistoryMeter) {
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

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(7),
        theme::alpha(
            theme::blend(theme::BG_ELEVATED, meter.accent, 0.10),
            if meter.focused { 214 } else { 184 },
        ),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(7),
        Stroke::new(
            1.0,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE, meter.accent, 0.34), 180),
        ),
        StrokeKind::Outside,
    );
    painter.rect_filled(track_rect, CornerRadius::same(2), theme::alpha(theme::FG_DIM, 52));
    if fill_width > 0.0 {
        painter.rect_filled(
            fill_rect,
            CornerRadius::same(2),
            theme::alpha(
                theme::blend(theme::ACCENT, meter.accent, 0.35),
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
            theme::FG_SOFT
        } else {
            theme::FG_DIM
        },
    );
}

#[profiling::function]
fn paint_attention_badge(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    severity: AttentionSeverity,
    summary: &str,
) {
    let color = attention_severity_color(severity);
    let icon = attention_severity_icon(severity);

    // Truncate the summary for display.
    let display_text = if summary.len() > 30 {
        let mut truncated = summary[..29].to_string();
        truncated.push('\u{2026}');
        truncated
    } else {
        summary.to_string()
    };
    let badge_text = format!("{icon} {display_text}");
    let font = egui::FontId::proportional(10.0);

    // Position the badge left of the history meter area.
    let history_badge = panel_history_badge_rect(titlebar_rect, close_rect);
    let badge_right = history_badge.min.x - 6.0;
    let text_galley = painter.layout_no_wrap(badge_text.clone(), font.clone(), color);
    let text_width = text_galley.size().x;
    let badge_width = text_width + 12.0;
    let badge_height: f32 = 18.0;
    let badge_left = (badge_right - badge_width).max(titlebar_rect.min.x + 60.0);
    let badge_rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_right - badge_left, badge_height),
    );

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
    );
    painter.text(
        Pos2::new(badge_left + 6.0, titlebar_rect.center().y),
        egui::Align2::LEFT_CENTER,
        badge_text,
        font,
        color,
    );
}

#[profiling::function]
fn paint_ssh_status_badge(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    has_history_meter: bool,
    status: SshConnectionStatus,
) {
    let color = match status {
        SshConnectionStatus::Connecting => Color32::from_rgb(249, 226, 175),
        SshConnectionStatus::Connected => Color32::from_rgb(166, 227, 161),
        SshConnectionStatus::Disconnected => theme::PALETTE_RED,
    };
    let badge_text = status.label();
    let font = egui::FontId::proportional(10.0);
    let badge_right = if has_history_meter {
        panel_history_badge_rect(titlebar_rect, close_rect).min.x - 6.0
    } else {
        close_rect.min.x - 8.0
    };
    let text_width = painter
        .layout_no_wrap(badge_text.to_string(), font.clone(), color)
        .size()
        .x;
    let badge_width = text_width + 16.0;
    let badge_height = 18.0;
    let badge_left = (badge_right - badge_width).max(titlebar_rect.min.x + 60.0);
    let badge_rect = Rect::from_min_size(
        Pos2::new(badge_left, titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_right - badge_left, badge_height),
    );

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 72),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(4),
        Stroke::new(1.0, theme::alpha(color, 140)),
        StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        badge_text,
        font,
        color,
    );
}

fn attention_severity_color(severity: AttentionSeverity) -> Color32 {
    match severity {
        AttentionSeverity::High => theme::PALETTE_RED,
        AttentionSeverity::Medium => theme::PALETTE_GREEN,
        AttentionSeverity::Low => theme::ACCENT,
    }
}

fn attention_severity_icon(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::High => "\u{26A0}",
        AttentionSeverity::Medium => "\u{2713}",
        AttentionSeverity::Low => "\u{2139}",
    }
}

fn panel_history_badge_rect(titlebar_rect: Rect, close_rect: Rect) -> Rect {
    let badge_size = Vec2::new(96.0, 20.0);
    Rect::from_center_size(
        Pos2::new(close_rect.min.x - (badge_size.x * 0.5) - 10.0, titlebar_rect.center().y),
        badge_size,
    )
}

pub(super) fn panel_title_content_rect(titlebar_rect: Rect, close_rect: Rect, has_workspace_accent: bool) -> Rect {
    let left = if has_workspace_accent {
        titlebar_rect.min.x + 26.0
    } else {
        titlebar_rect.min.x + 12.0
    };
    let badge_rect = panel_history_badge_rect(titlebar_rect, close_rect);
    let right = (badge_rect.min.x - 12.0).max(left + 1.0);

    Rect::from_min_max(
        Pos2::new(left, titlebar_rect.min.y + 2.0),
        Pos2::new(right, titlebar_rect.max.y - 2.0),
    )
}

pub(super) fn show_inline_rename_editor(
    ui: &mut egui::Ui,
    rect: Rect,
    buffer: &mut String,
    font: egui::FontId,
) -> RenameEditAction {
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let edit = egui::TextEdit::singleline(buffer)
        .font(font)
        .text_color(theme::FG)
        .frame(false)
        .desired_width(rect.width())
        .margin(Margin::ZERO);
    let response = ui.add(edit);
    if !response.has_focus() {
        response.request_focus();
    }

    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let lost_focus = response.lost_focus();

    if escape {
        RenameEditAction::Cancel
    } else if enter || lost_focus {
        RenameEditAction::Commit
    } else {
        RenameEditAction::None
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, Pos2, Rect};

    use super::{
        focus_ring_stroke, panel_border_stroke, panel_fill, panel_title_color, panel_titlebar_fill,
        title_focus_indicator_rect,
    };

    #[test]
    fn focused_panel_style_is_more_prominent() {
        let accent = Color32::from_rgb(137, 180, 250);

        assert!(focus_ring_stroke(accent, true).is_some());
        assert_eq!(focus_ring_stroke(accent, false), None);
        assert!(panel_border_stroke(accent, true).width > panel_border_stroke(accent, false).width);
        assert_ne!(panel_fill(accent, true), panel_fill(accent, false));
        assert_ne!(panel_titlebar_fill(accent, true), panel_titlebar_fill(accent, false));
        assert_ne!(panel_title_color(true), panel_title_color(false));
    }

    #[test]
    fn title_focus_indicator_stays_inside_titlebar() {
        let titlebar_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(210.0, 54.0));
        let indicator = title_focus_indicator_rect(titlebar_rect);

        assert!(titlebar_rect.contains(indicator.min));
        assert!(titlebar_rect.contains(indicator.max - indicator.size() * 0.01));
        assert!(indicator.width() > 0.0);
        assert!(indicator.height() > 0.0);
    }
}
