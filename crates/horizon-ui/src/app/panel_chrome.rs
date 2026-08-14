use egui::{Align, Color32, CornerRadius, Layout, Margin, Pos2, Rect, Stroke, StrokeKind, UiBuilder, Vec2};
use horizon_core::{AttentionSeverity, PanelId, PanelKind, SshConnectionStatus, agent_definition};

use crate::text::single_line_label_job;
use crate::theme;

use super::RenameEditAction;
use super::speech::MicState;

mod badges;

use badges::{
    HistoryMeter, paint_attention_badge, paint_history_meter, paint_ssh_status_badge, panel_history_badge_rect,
    title_right_boundary,
};

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
    /// Speech mic control; `None` when speech input is not enabled.
    pub mic: Option<MicControl>,
}

#[derive(Clone, Copy)]
pub(super) struct MicControl {
    pub rect: Rect,
    pub hovered: bool,
    pub state: MicState,
}

impl PanelChrome<'_> {
    /// Leftmost titlebar control: badges and the title stop left of this.
    fn controls_anchor(&self) -> Rect {
        self.mic.map_or(self.close_rect, |mic| mic.rect)
    }
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
        paint_attention_badge(
            &painter,
            chrome.titlebar_rect,
            chrome.controls_anchor(),
            chrome.scrollback_limit > 0,
            chrome.ssh_status.is_some(),
            *severity,
            summary,
        );
    }
    if let Some(status) = chrome.ssh_status {
        paint_ssh_status_badge(
            &painter,
            chrome.titlebar_rect,
            chrome.controls_anchor(),
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
                close_rect: chrome.controls_anchor(),
                accent,
                history_size: chrome.history_size,
                scrollback_limit: chrome.scrollback_limit,
                focused: chrome.focused,
            },
        );
    }

    if let Some(mic) = chrome.mic {
        paint_mic_control(ui, &painter, mic);
    }
    paint_close_and_resize_controls(&painter, chrome.close_rect, chrome.resize_rect, chrome.close_hovered);
}

/// Painter-drawn microphone glyph: capsule body, U-shaped cradle, and stem.
/// Pulses red while recording and amber while a transcription is in flight.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pulse alpha math stays within [0, 255]"
)]
fn paint_mic_control(ui: &egui::Ui, painter: &egui::Painter, mic: MicControl) {
    // Cradle polyline offsets (radius 4.5, 22.5° steps) — precomputed so the
    // hot chrome path allocates nothing per frame.
    const CRADLE_OFFSETS: [Vec2; 9] = [
        Vec2::new(4.5, 0.0),
        Vec2::new(4.157, 1.722),
        Vec2::new(3.182, 3.182),
        Vec2::new(1.722, 4.157),
        Vec2::new(0.0, 4.5),
        Vec2::new(-1.722, 4.157),
        Vec2::new(-3.182, 3.182),
        Vec2::new(-4.157, 1.722),
        Vec2::new(-4.5, 0.0),
    ];
    let center = mic.rect.center();
    let time = ui.input(|input| input.time);
    let pulse = (0.5 + 0.5 * (time * 5.0).sin() as f32).clamp(0.0, 1.0);

    let color = match mic.state {
        MicState::Idle => {
            if mic.hovered {
                theme::FG()
            } else {
                theme::alpha(theme::FG_DIM(), 140)
            }
        }
        MicState::Recording => {
            let base = Color32::from_rgb(232, 72, 72);
            painter.circle_stroke(
                center,
                9.0 + pulse * 2.0,
                Stroke::new(1.2_f32, theme::alpha(base, 90 + (pulse * 120.0) as u8)),
            );
            base
        }
        MicState::Busy => theme::alpha(theme::PALETTE_YELLOW(), 140 + (pulse * 100.0) as u8),
    };

    // Body capsule.
    painter.rect_filled(
        Rect::from_center_size(center + Vec2::new(0.0, -2.5), Vec2::new(5.0, 8.0)),
        CornerRadius::same(3),
        color,
    );
    // Cradle: semicircle under the body.
    let cradle_center = center + Vec2::new(0.0, 0.5);
    let cradle_stroke = Stroke::new(1.3_f32, color);
    for pair in CRADLE_OFFSETS.windows(2) {
        painter.line_segment([cradle_center + pair[0], cradle_center + pair[1]], cradle_stroke);
    }
    // Stem.
    painter.line_segment(
        [center + Vec2::new(0.0, 5.0), center + Vec2::new(0.0, 7.0)],
        Stroke::new(1.3_f32, color),
    );
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

#[profiling::function]
fn paint_truncated_title(painter: &egui::Painter, title: &str, x: f32, center_y: f32, max_width: f32, focused: bool) {
    let job = single_line_label_job(
        title,
        &egui::FontId::proportional(13.0),
        panel_title_color(focused),
        max_width,
    );
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
