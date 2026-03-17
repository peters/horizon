use egui::{Align, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect, Stroke, StrokeKind, UiBuilder, Vec2};
use horizon_core::{AttentionSeverity, PanelId, PanelKind};

use crate::theme;

use super::RenameEditAction;
use super::util::{format_compact_count, usize_to_f32};

pub(super) fn panel_kind_icon(kind: PanelKind, workspace_color: Color32, focused: bool) -> (&'static str, Color32) {
    match kind {
        PanelKind::Shell | PanelKind::Command => (">_", theme::alpha(workspace_color, if focused { 200 } else { 80 })),
        PanelKind::Codex => (
            "CX",
            theme::alpha(Color32::from_rgb(116, 162, 247), if focused { 220 } else { 120 }),
        ),
        PanelKind::Claude => (
            "CC",
            theme::alpha(Color32::from_rgb(203, 166, 247), if focused { 220 } else { 120 }),
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
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_panel_chrome(
    ui: &mut egui::Ui,
    panel_id: PanelId,
    panel_rect: Rect,
    titlebar_rect: Rect,
    close_rect: Rect,
    resize_rect: Rect,
    title: Option<&str>,
    history_size: usize,
    scrollback_limit: usize,
    focused: bool,
    close_hovered: bool,
    workspace_accent: Option<Color32>,
    attention_badge: Option<&(AttentionSeverity, String)>,
) {
    let painter = ui.painter_at(panel_rect);
    let accent = workspace_accent.unwrap_or(if focused { theme::ACCENT } else { theme::BORDER_STRONG });

    painter.rect_filled(panel_rect, CornerRadius::same(16), theme::PANEL_BG);
    painter.rect_stroke(
        panel_rect,
        CornerRadius::same(16),
        Stroke::new(1.2, theme::panel_border(accent, focused)),
        StrokeKind::Outside,
    );
    painter.rect_filled(
        titlebar_rect,
        CornerRadius::same(16),
        theme::blend(theme::PANEL_BG_ALT, accent, if focused { 0.18 } else { 0.10 }),
    );

    if let Some(title) = title {
        let title_x = if let Some(color) = workspace_accent {
            painter.circle_filled(
                Pos2::new(titlebar_rect.min.x + 14.0, titlebar_rect.center().y),
                4.5,
                color,
            );
            titlebar_rect.min.x + 26.0
        } else {
            titlebar_rect.min.x + 12.0
        };
        painter.text(
            Pos2::new(title_x, titlebar_rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(13.0),
            theme::FG,
        );
    }

    if let Some((severity, summary)) = attention_badge {
        paint_attention_badge(&painter, titlebar_rect, close_rect, *severity, summary);
    }

    if scrollback_limit > 0 {
        paint_history_meter(
            ui,
            &painter,
            panel_id,
            titlebar_rect,
            close_rect,
            accent,
            history_size,
            scrollback_limit,
            focused,
        );
    }

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

#[allow(clippy::too_many_arguments)]
fn paint_history_meter(
    ui: &egui::Ui,
    painter: &egui::Painter,
    panel_id: PanelId,
    titlebar_rect: Rect,
    close_rect: Rect,
    accent: Color32,
    history_size: usize,
    scrollback_limit: usize,
    focused: bool,
) {
    let badge_rect = panel_history_badge_rect(titlebar_rect, close_rect);
    let track_rect = Rect::from_min_max(
        Pos2::new(badge_rect.min.x + 8.0, badge_rect.max.y - 5.0),
        Pos2::new(badge_rect.max.x - 8.0, badge_rect.max.y - 3.0),
    );
    let ratio = if scrollback_limit == 0 {
        0.0
    } else {
        (usize_to_f32(history_size) / usize_to_f32(scrollback_limit)).clamp(0.0, 1.0)
    };
    let animated_ratio = ui
        .ctx()
        .animate_value_with_time(Id::new(("panel_history_ratio", panel_id.0)), ratio, 0.16);
    let fill_width = track_rect.width() * animated_ratio.clamp(0.0, 1.0);
    let fill_rect = Rect::from_min_max(
        track_rect.min,
        Pos2::new(track_rect.min.x + fill_width, track_rect.max.y),
    );
    let history_text = format!(
        "{}/{}",
        format_compact_count(history_size),
        format_compact_count(scrollback_limit)
    );

    painter.rect_filled(
        badge_rect,
        CornerRadius::same(7),
        theme::alpha(
            theme::blend(theme::BG_ELEVATED, accent, 0.10),
            if focused { 214 } else { 184 },
        ),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(7),
        Stroke::new(1.0, theme::alpha(theme::blend(theme::BORDER_SUBTLE, accent, 0.34), 180)),
        StrokeKind::Outside,
    );
    painter.rect_filled(track_rect, CornerRadius::same(2), theme::alpha(theme::FG_DIM, 52));
    if fill_width > 0.0 {
        painter.rect_filled(
            fill_rect,
            CornerRadius::same(2),
            theme::alpha(
                theme::blend(theme::ACCENT, accent, 0.35),
                if focused { 224 } else { 188 },
            ),
        );
    }
    painter.text(
        Pos2::new(badge_rect.center().x, badge_rect.center().y - 2.0),
        egui::Align2::CENTER_CENTER,
        history_text,
        egui::FontId::monospace(10.5),
        if history_size > 0 {
            theme::FG_SOFT
        } else {
            theme::FG_DIM
        },
    );
}

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
