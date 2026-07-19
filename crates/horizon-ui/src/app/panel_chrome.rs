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

struct AttentionBadgeLayout {
    rect: Rect,
    text: String,
}

struct ChromeBadgeLayout {
    attention: Option<AttentionBadgeLayout>,
    ssh: Option<Rect>,
    title_right: f32,
}

fn panel_accent(workspace_accent: Option<Color32>, focused: bool) -> Color32 {
    workspace_accent.unwrap_or(if focused {
        theme::ACCENT()
    } else {
        theme::BORDER_STRONG()
    })
}

fn panel_fill(accent: Color32, focused: bool) -> Color32 {
    theme::blend(theme::PANEL_BG(), accent, if focused { 0.06 } else { 0.0 })
}

fn panel_border_stroke(accent: Color32, focused: bool) -> Stroke {
    Stroke::new(
        if focused { 1.8_f32 } else { 1.2_f32 },
        theme::panel_border(accent, focused),
    )
}

fn panel_titlebar_fill(accent: Color32, focused: bool) -> Color32 {
    theme::blend(theme::PANEL_BG_ALT(), accent, if focused { 0.28 } else { 0.10 })
}

fn panel_title_color(focused: bool) -> Color32 {
    if focused { theme::FG() } else { theme::FG_SOFT() }
}

fn focus_ring_stroke(accent: Color32, focused: bool) -> Option<Stroke> {
    focused.then(|| Stroke::new(3.0_f32, theme::alpha(theme::blend(theme::ACCENT(), accent, 0.35), 56)))
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
            panel_kind_label_color(Color32::from_rgb(r, g, b), focused),
        );
    }

    match kind {
        PanelKind::Shell | PanelKind::Command => (">_", panel_kind_label_color(workspace_color, focused)),
        PanelKind::Ssh => ("SSH", panel_kind_label_color(theme::PALETTE_YELLOW(), focused)),
        PanelKind::Editor => ("MD", panel_kind_label_color(theme::PALETTE_GREEN(), focused)),
        PanelKind::GitChanges => ("GC", panel_kind_label_color(theme::PALETTE_YELLOW(), focused)),
        PanelKind::Usage => ("US", panel_kind_label_color(theme::PALETTE_YELLOW(), focused)),
        PanelKind::Codex
        | PanelKind::Claude
        | PanelKind::OpenCode
        | PanelKind::Gemini
        | PanelKind::KiloCode
        | PanelKind::Pi => {
            unreachable!()
        }
    }
}

fn panel_kind_label_color(base: Color32, focused: bool) -> Color32 {
    let adjusted = match theme::current_theme() {
        theme::ResolvedTheme::Dark => base,
        theme::ResolvedTheme::Light => theme::ensure_terminal_text_contrast(base, theme::PANEL_BG_ALT()),
    };
    let alpha = match theme::current_theme() {
        theme::ResolvedTheme::Dark => {
            if focused {
                220
            } else {
                120
            }
        }
        theme::ResolvedTheme::Light => {
            if focused {
                255
            } else {
                228
            }
        }
    };

    theme::alpha(adjusted, alpha)
}

#[profiling::function]
pub(super) fn paint_panel_chrome(ui: &mut egui::Ui, chrome: PanelChrome<'_>) {
    let painter = ui.painter_at(chrome.panel_rect);
    let accent = panel_chrome_accent(chrome.kind, chrome.workspace_accent, chrome.focused);
    let badge_layout = chrome_badge_layout(
        &painter,
        chrome.titlebar_rect,
        chrome.close_rect,
        chrome.scrollback_limit,
        chrome.attention_badge,
        chrome.ssh_status,
    );

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
        let max_width = (badge_layout.title_right - title_x).max(0.0);
        paint_truncated_title(
            &painter,
            title,
            title_x,
            chrome.titlebar_rect.center().y,
            max_width,
            chrome.focused,
        );
    }

    if let (Some((severity, _)), Some(attention)) = (chrome.attention_badge, badge_layout.attention.as_ref()) {
        paint_attention_badge(&painter, attention, *severity);
    }
    if let (Some(status), Some(rect)) = (chrome.ssh_status, badge_layout.ssh) {
        paint_ssh_status_badge(&painter, rect, status);
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
            theme::BTN_CLOSE()
        } else {
            theme::alpha(theme::FG_DIM(), 140)
        },
    );

    let handle_stroke = Stroke::new(1.0_f32, theme::alpha(theme::FG_DIM(), 170));
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

fn chrome_badge_layout(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    scrollback_limit: usize,
    attention_badge: Option<&(AttentionSeverity, String)>,
    ssh_status: Option<SshConnectionStatus>,
) -> ChromeBadgeLayout {
    let history_rect = (scrollback_limit > 0).then(|| panel_history_badge_rect(titlebar_rect, close_rect));
    let mut right = history_rect.map_or(close_rect.min.x - 8.0, |rect| rect.min.x - 6.0);

    let ssh = ssh_status.and_then(|status| {
        let font = egui::FontId::proportional(10.0);
        let text_width = painter
            .layout_no_wrap(status.label().to_string(), font, ssh_status_color(status))
            .size()
            .x;
        let rect = reserve_badge_rect(titlebar_rect, right, text_width + 16.0, 18.0)?;
        right = rect.min.x - 6.0;
        Some(rect)
    });

    let attention = attention_badge.and_then(|(severity, summary)| {
        let text = attention_badge_text(*severity, summary);
        let font = egui::FontId::proportional(10.0);
        let text_width = painter
            .layout_no_wrap(text.clone(), font, attention_severity_color(*severity))
            .size()
            .x;
        let rect = reserve_badge_rect(titlebar_rect, right, text_width + 12.0, 18.0)?;
        right = rect.min.x - 6.0;
        Some(AttentionBadgeLayout { rect, text })
    });

    let title_right = if attention.is_some() || ssh.is_some() || history_rect.is_some() {
        right - 2.0
    } else {
        close_rect.min.x - 12.0
    };
    ChromeBadgeLayout {
        attention,
        ssh,
        title_right,
    }
}

fn reserve_badge_rect(titlebar_rect: Rect, right: f32, desired_width: f32, height: f32) -> Option<Rect> {
    let left_limit = titlebar_rect.min.x + 60.0;
    if right <= left_limit {
        return None;
    }
    let left = (right - desired_width).max(left_limit);
    Some(Rect::from_min_max(
        Pos2::new(left, titlebar_rect.center().y - height * 0.5),
        Pos2::new(right, titlebar_rect.center().y + height * 0.5),
    ))
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
            theme::blend(theme::BG_ELEVATED(), meter.accent, 0.10),
            if meter.focused { 214 } else { 184 },
        ),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(7),
        Stroke::new(
            1.0_f32,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE(), meter.accent, 0.34), 180),
        ),
        StrokeKind::Outside,
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
fn paint_attention_badge(painter: &egui::Painter, layout: &AttentionBadgeLayout, severity: AttentionSeverity) {
    let color = attention_severity_color(severity);
    let font = egui::FontId::proportional(10.0);

    painter.rect_filled(
        layout.rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
    );
    painter.with_clip_rect(layout.rect.shrink2(Vec2::new(4.0, 0.0))).text(
        Pos2::new(layout.rect.min.x + 6.0, layout.rect.center().y),
        egui::Align2::LEFT_CENTER,
        &layout.text,
        font,
        color,
    );
}

fn attention_badge_text(severity: AttentionSeverity, summary: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 30;

    let mut chars = summary.chars();
    let prefix: String = chars.by_ref().take(MAX_SUMMARY_CHARS).collect();
    let display_text = if chars.next().is_some() {
        let mut truncated: String = prefix.chars().take(MAX_SUMMARY_CHARS - 1).collect();
        truncated.push('\u{2026}');
        truncated
    } else {
        prefix
    };
    format!("{} {display_text}", attention_severity_icon(severity))
}

#[profiling::function]
fn paint_ssh_status_badge(painter: &egui::Painter, badge_rect: Rect, status: SshConnectionStatus) {
    let color = ssh_status_color(status);
    let badge_text = status.label();
    let font = egui::FontId::proportional(10.0);
    painter.rect_filled(
        badge_rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 72),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(4),
        Stroke::new(1.0_f32, theme::alpha(color, 140)),
        StrokeKind::Inside,
    );
    painter.with_clip_rect(badge_rect.shrink2(Vec2::new(2.0, 0.0))).text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        badge_text,
        font,
        color,
    );
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

fn panel_history_badge_rect(titlebar_rect: Rect, close_rect: Rect) -> Rect {
    let badge_size = Vec2::new(96.0, 20.0);
    Rect::from_center_size(
        Pos2::new(close_rect.min.x - (badge_size.x * 0.5) - 10.0, titlebar_rect.center().y),
        badge_size,
    )
}

pub(super) fn panel_title_content_rect(
    painter: &egui::Painter,
    titlebar_rect: Rect,
    close_rect: Rect,
    has_workspace_accent: bool,
    scrollback_limit: usize,
    attention_badge: Option<&(AttentionSeverity, String)>,
    ssh_status: Option<SshConnectionStatus>,
) -> Rect {
    let left = if has_workspace_accent {
        titlebar_rect.min.x + 26.0
    } else {
        titlebar_rect.min.x + 12.0
    };
    let badge_layout = chrome_badge_layout(
        painter,
        titlebar_rect,
        close_rect,
        scrollback_limit,
        attention_badge,
        ssh_status,
    );
    let right = badge_layout.title_right.max(left + 1.0);

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
        .text_color(theme::FG())
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
        attention_badge_text, focus_ring_stroke, panel_border_stroke, panel_fill, panel_title_color,
        panel_titlebar_fill, reserve_badge_rect, title_focus_indicator_rect,
    };
    use horizon_core::AttentionSeverity;

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

    #[test]
    fn attention_badge_text_truncates_unicode_at_character_boundaries() {
        let text = attention_badge_text(AttentionSeverity::High, &"\u{1F6A8}".repeat(31));

        assert!(text.ends_with('\u{2026}'));
        assert_eq!(text.chars().skip(2).count(), 30);
    }

    #[test]
    fn adjacent_badge_reservations_do_not_overlap() {
        let titlebar = Rect::from_min_max(Pos2::ZERO, Pos2::new(320.0, 34.0));
        let ssh = reserve_badge_rect(titlebar, 182.0, 80.0, 18.0).expect("ssh badge");
        let attention = reserve_badge_rect(titlebar, ssh.min.x - 6.0, 160.0, 18.0).expect("attention badge");

        assert!(attention.max.x <= ssh.min.x - 6.0);
        assert!(titlebar.contains(attention.min));
        assert!(titlebar.contains(ssh.max));
    }
}
