use egui::{Align, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect, Stroke, StrokeKind, UiBuilder, Vec2};
use horizon_core::{AgentStatus, AttentionSeverity, PanelId, PanelKind, SshConnectionStatus, agent_definition};

use crate::theme;

use super::RenameEditAction;
use super::speech::MicState;
use super::util::{format_compact_count, short_session_id, usize_to_f32};

#[derive(Clone, Copy)]
pub(super) struct PanelChrome<'a> {
    pub panel_id: PanelId,
    pub kind: PanelKind,
    pub agent_status: AgentStatus,
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
    /// Current agent session id; when `Some` a short-id badge is painted in
    /// the titlebar so the panel can be matched against rebind/resume lists.
    pub session_id: Option<&'a str>,
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
        | PanelKind::Pi
        | PanelKind::Grok => {
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

    // Measured once per frame: drives the session badge's overlap guard
    // and the attention paint, so the two can never diverge.
    let attention_geom = attention_badge_geometry(&painter, &chrome);

    if let Some(title) = chrome.title {
        if let Some(color) = chrome.workspace_accent {
            painter.circle_filled(
                Pos2::new(chrome.titlebar_rect.min.x + 14.0, chrome.titlebar_rect.center().y),
                if chrome.focused { 5.0 } else { 4.5 },
                theme::alpha(color, if chrome.focused { 240 } else { 180 }),
            );
        }
        let title_x = title_start_x(&chrome);
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

    if let Some(session_id) = chrome.session_id
        && let Some(badge_rect) = session_badge_rect(&chrome)
        // A long notification summary measures wider than the fixed titlebar
        // reserve; the attention badge would paint over the session pill.
        && attention_geom.as_ref().is_none_or(|(rect, ..)| {
            session_badge_clears_attention_badge(badge_rect, rect.min.x)
        })
    {
        paint_session_badge(
            &painter,
            badge_rect,
            accent,
            short_session_id(session_id),
            chrome.focused,
        );
    }

    if let Some(geometry) = &attention_geom {
        paint_attention_badge(&painter, geometry);
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
    if chrome.agent_status == AgentStatus::Working {
        paint_working_indicator(
            ui,
            &painter,
            chrome.titlebar_rect,
            badges_left_boundary(&chrome),
            chrome.kind,
            chrome.focused,
        );
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

/// Compute the right x boundary where the title text must stop, accounting
/// for all badges (history meter, SSH status, attention) that sit to its
/// right, the working slot, and the session badge when one is shown.
fn title_right_boundary(chrome: &PanelChrome<'_>) -> f32 {
    let right = if chrome.agent_status == AgentStatus::Working {
        badges_left_boundary(chrome) - working_indicator_reserve()
    } else {
        badges_left_boundary(chrome)
    };
    match session_badge_rect(chrome) {
        Some(badge) => badge.min.x.min(right),
        None => right,
    }
}

/// Fixed-width pill showing the short session id, positioned left of the
/// (always reserved) working slot so it does not shift when the agent starts
/// working.
const SESSION_BADGE_WIDTH: f32 = 64.0;
const SESSION_BADGE_HEIGHT: f32 = 18.0;
const SESSION_BADGE_TITLE_GAP: f32 = 8.0;
/// Minimum title text space kept to the left of the badge, measured from the
/// title's start (behind the workspace accent dot when one is painted);
/// panels that cannot spare it hide the badge instead of covering the dot.
const SESSION_BADGE_MIN_TITLE_SPACE: f32 = 12.0;

/// X position where the title text begins — behind the workspace accent dot
/// when one is painted.
fn title_start_x(chrome: &PanelChrome<'_>) -> f32 {
    if chrome.workspace_accent.is_some() {
        chrome.titlebar_rect.min.x + 26.0
    } else {
        chrome.titlebar_rect.min.x + 12.0
    }
}

fn session_badge_left(chrome: &PanelChrome<'_>) -> f32 {
    badges_left_boundary(chrome) - working_indicator_reserve() - SESSION_BADGE_TITLE_GAP - SESSION_BADGE_WIDTH
}

fn session_badge_rect(chrome: &PanelChrome<'_>) -> Option<Rect> {
    let fits = chrome.session_id.is_some()
        && session_badge_left(chrome) >= title_start_x(chrome) + SESSION_BADGE_MIN_TITLE_SPACE;
    fits.then(|| {
        let left = session_badge_left(chrome);
        let center_y = chrome.titlebar_rect.center().y;
        Rect::from_min_max(
            Pos2::new(left, center_y - SESSION_BADGE_HEIGHT * 0.5),
            Pos2::new(left + SESSION_BADGE_WIDTH, center_y + SESSION_BADGE_HEIGHT * 0.5),
        )
    })
}

/// Left edge of the titlebar's right-side badge cluster (history meter,
/// SSH status, attention).
fn badges_left_boundary(chrome: &PanelChrome<'_>) -> f32 {
    let anchor = chrome.controls_anchor();
    let mut right = anchor.min.x - 12.0;
    if chrome.scrollback_limit > 0 {
        right = panel_history_badge_rect(chrome.titlebar_rect, anchor).min.x - 8.0;
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

/// Three-dot "working" indicator mirroring the agent TUI's own busy
/// spinner, pulsing left-to-right while the agent executes.
const WORKING_DOT_RADIUS: f32 = 2.0;
const WORKING_DOT_SPACING: f32 = 6.0;
const WORKING_BADGE_GAP: f32 = 6.0;
const WORKING_TITLE_GAP: f32 = 8.0;
const WORKING_PULSE_HZ: f32 = 1.0;

const fn working_indicator_width() -> f32 {
    WORKING_DOT_RADIUS * 6.0 + WORKING_DOT_SPACING * 2.0
}

const fn working_indicator_reserve() -> f32 {
    working_indicator_width() + WORKING_BADGE_GAP + WORKING_TITLE_GAP
}

fn working_indicator_base_color(kind: PanelKind, focused: bool) -> Color32 {
    let base = match agent_definition(kind) {
        Some(definition) => {
            let [r, g, b] = definition.accent_rgb;
            Color32::from_rgb(r, g, b)
        }
        None => theme::ACCENT(),
    };
    let base = match theme::current_theme() {
        theme::ResolvedTheme::Dark => base,
        theme::ResolvedTheme::Light => theme::ensure_terminal_text_contrast(base, theme::PANEL_BG_ALT()),
    };
    theme::alpha(base, if focused { 235 } else { 175 })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "time-to-f32 and wave-scaled alpha both stay within representable range"
)]
#[profiling::function]
fn paint_working_indicator(
    ui: &egui::Ui,
    painter: &egui::Painter,
    titlebar_rect: Rect,
    badges_left: f32,
    kind: PanelKind,
    focused: bool,
) {
    let time = ui.input(|input| input.time as f32);
    let base = working_indicator_base_color(kind, focused);
    let base_alpha = f32::from(base.a());
    let left = badges_left - WORKING_BADGE_GAP - working_indicator_width();
    let step = WORKING_DOT_RADIUS * 2.0 + WORKING_DOT_SPACING;
    let center_y = titlebar_rect.center().y;

    for dot in [0.0_f32, 1.0, 2.0] {
        // A wave travelling left to right, like the agent's typing dots.
        let phase = time * WORKING_PULSE_HZ - dot / 3.0;
        let wave = (0.5 + 0.5 * (phase * std::f32::consts::TAU).sin()).clamp(0.0, 1.0);
        let alpha = (base_alpha * (0.30 + 0.70 * wave)) as u8;
        let x = left + WORKING_DOT_RADIUS + dot * step;
        painter.circle_filled(Pos2::new(x, center_y), WORKING_DOT_RADIUS, theme::alpha(base, alpha));
    }
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

/// Painter-drawn pill showing the short session id of the agent bound to the
/// panel, styled after the history meter badge.
#[profiling::function]
fn paint_session_badge(painter: &egui::Painter, badge_rect: Rect, accent: Color32, session_id: &str, focused: bool) {
    painter.rect_filled(
        badge_rect,
        CornerRadius::same(4),
        theme::alpha(
            theme::blend(theme::BG_ELEVATED(), accent, 0.10),
            if focused { 200 } else { 160 },
        ),
    );
    painter.rect_stroke(
        badge_rect,
        CornerRadius::same(4),
        Stroke::new(
            1.0_f32,
            theme::alpha(theme::blend(theme::BORDER_SUBTLE(), accent, 0.34), 150),
        ),
        StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        session_id,
        egui::FontId::monospace(10.5),
        if focused { theme::FG_SOFT() } else { theme::FG_DIM() },
    );
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
fn attention_badge_geometry(
    painter: &egui::Painter,
    chrome: &PanelChrome<'_>,
) -> Option<(Rect, String, egui::FontId, Color32)> {
    let (severity, summary) = chrome.attention_badge?;
    let summary: &str = summary;
    let color = attention_severity_color(*severity);
    let icon = attention_severity_icon(*severity);

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
    let history_badge = panel_history_badge_rect(chrome.titlebar_rect, chrome.controls_anchor());
    let badge_right = history_badge.min.x - 6.0;
    let text_galley = painter.layout_no_wrap(badge_text.clone(), font.clone(), color);
    let text_width = text_galley.size().x;
    let badge_width = text_width + 12.0;
    let badge_height: f32 = 18.0;
    let badge_left = (badge_right - badge_width).max(chrome.titlebar_rect.min.x + 60.0);
    let rect = Rect::from_min_size(
        Pos2::new(badge_left, chrome.titlebar_rect.center().y - badge_height * 0.5),
        Vec2::new(badge_right - badge_left, badge_height),
    );
    Some((rect, badge_text, font, color))
}

/// The session badge is hidden when the painted attention badge would cover
/// it — long summaries measure wider than the fixed titlebar reserve.
fn session_badge_clears_attention_badge(session_badge: Rect, attention_left: f32) -> bool {
    session_badge.max.x <= attention_left
}

#[profiling::function]
fn paint_attention_badge(painter: &egui::Painter, geometry: &(Rect, String, egui::FontId, Color32)) {
    let (rect, badge_text, font, color) = geometry;

    painter.rect_filled(
        *rect,
        CornerRadius::same(4),
        Color32::from_rgba_unmultiplied(color.r() / 6, color.g() / 6, color.b() / 6, 60),
    );
    painter.text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        badge_text.clone(),
        font.clone(),
        *color,
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
    let color = ssh_status_color(status);
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
        Stroke::new(1.0_f32, theme::alpha(color, 140)),
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

pub(super) fn panel_title_content_rect(chrome: &PanelChrome<'_>) -> Rect {
    let titlebar = chrome.titlebar_rect;
    let left = title_start_x(chrome);
    // Same right-side boundary as the painted title: the badge cluster (history
    // meter, SSH, attention, mic) narrowed to the session badge's left edge
    // when the badge is painted.
    let mut right = badges_left_boundary(chrome);
    if let Some(badge) = session_badge_rect(chrome) {
        right = badge.min.x;
    }
    let right = right.max(left + 1.0);

    Rect::from_min_max(
        Pos2::new(left, titlebar.min.y + 2.0),
        Pos2::new(right, titlebar.max.y - 2.0),
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
        AgentStatus, PanelChrome, SESSION_BADGE_MIN_TITLE_SPACE, SESSION_BADGE_TITLE_GAP, SESSION_BADGE_WIDTH,
        WORKING_BADGE_GAP, badges_left_boundary, focus_ring_stroke, panel_border_stroke, panel_fill, panel_title_color,
        panel_title_content_rect, panel_titlebar_fill, session_badge_clears_attention_badge, session_badge_rect,
        title_focus_indicator_rect, title_right_boundary, working_indicator_reserve, working_indicator_width,
    };

    /// The boundary math is exact constant arithmetic, so an epsilon of one
    /// ULP keeps `assert_eq!`-level strictness without tripping `float_cmp`.
    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= f32::EPSILON
    }

    fn test_chrome(agent_status: AgentStatus) -> PanelChrome<'static> {
        let panel_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(600.0, 400.0));
        let titlebar_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(600.0, 54.0));
        let close_rect = Rect::from_center_size(Pos2::new(582.0, 37.0), egui::Vec2::splat(16.0));
        PanelChrome {
            panel_id: horizon_core::PanelId(1),
            kind: horizon_core::PanelKind::Pi,
            agent_status,
            panel_rect,
            titlebar_rect,
            close_rect,
            resize_rect: panel_rect,
            title: Some("Smoke"),
            history_size: 100,
            scrollback_limit: 24_000,
            focused: true,
            close_hovered: false,
            workspace_accent: Some(Color32::from_rgb(137, 180, 250)),
            attention_badge: None,
            ssh_status: None,
            session_id: None,
            mic: None,
        }
    }

    fn test_chrome_with_session(agent_status: AgentStatus, session_id: Option<&'static str>) -> PanelChrome<'static> {
        let mut chrome = test_chrome(agent_status);
        chrome.session_id = session_id;
        chrome
    }

    /// Panel at a given titlebar width with the session badge and, optionally,
    /// an attention badge — 320 px is the supported minimum panel width.
    fn chrome_at_width_with_session(
        agent_status: AgentStatus,
        titlebar_width: f32,
        with_attention: bool,
    ) -> PanelChrome<'static> {
        let mut chrome = test_chrome_with_session(agent_status, Some("session-42"));
        let right = 10.0 + titlebar_width;
        chrome.panel_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(right, 400.0));
        chrome.titlebar_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(right, 54.0));
        chrome.close_rect = Rect::from_center_size(Pos2::new(right - 18.0, 37.0), egui::Vec2::splat(16.0));
        if with_attention {
            static ATTENTION: std::sync::OnceLock<(super::AttentionSeverity, String)> = std::sync::OnceLock::new();
            chrome.attention_badge =
                Some(ATTENTION.get_or_init(|| (super::AttentionSeverity::Low, "Waiting for input".to_string())));
        }
        chrome
    }

    #[test]
    fn working_indicator_reserves_titlebar_space() {
        let idle = test_chrome(AgentStatus::Idle);
        let working = test_chrome(AgentStatus::Working);

        assert!(approx_eq(title_right_boundary(&idle), badges_left_boundary(&idle)));
        assert!(approx_eq(
            title_right_boundary(&working),
            badges_left_boundary(&working) - working_indicator_reserve()
        ));
        assert!(title_right_boundary(&working) < title_right_boundary(&idle));
    }

    #[test]
    fn session_badge_reserves_titlebar_space_left_of_working_slot() {
        let plain = test_chrome(AgentStatus::Idle);
        let idle = test_chrome_with_session(AgentStatus::Idle, Some("session-42"));
        let working = test_chrome_with_session(AgentStatus::Working, Some("session-42"));

        let expected =
            badges_left_boundary(&idle) - working_indicator_reserve() - SESSION_BADGE_TITLE_GAP - SESSION_BADGE_WIDTH;
        assert!(approx_eq(title_right_boundary(&idle), expected));
        assert!(approx_eq(title_right_boundary(&working), expected));
        assert!(title_right_boundary(&idle) < title_right_boundary(&plain));
    }

    #[test]
    fn session_badge_rect_sits_left_of_working_slot_and_stays_stable() {
        let idle = test_chrome_with_session(AgentStatus::Idle, Some("session-42"));
        let working = test_chrome_with_session(AgentStatus::Working, Some("session-42"));
        let Some(idle_badge) = session_badge_rect(&idle) else {
            panic!("expected session badge rect");
        };
        let Some(working_badge) = session_badge_rect(&working) else {
            panic!("expected session badge rect while working");
        };

        assert!(
            idle_badge == working_badge,
            "badge must not shift while the agent works"
        );
        let badges_left = badges_left_boundary(&idle);
        let working_left = badges_left - WORKING_BADGE_GAP - working_indicator_width();
        assert!(
            idle_badge.max.x < working_left,
            "badge must sit left of the working slot"
        );
        assert!(approx_eq(idle_badge.width(), SESSION_BADGE_WIDTH));
        assert!(idle.titlebar_rect.contains(idle_badge.center()));
        assert!(session_badge_rect(&test_chrome(AgentStatus::Idle)).is_none());
    }

    #[test]
    fn session_badge_hides_when_titlebar_is_too_narrow() {
        let idle = chrome_at_width_with_session(AgentStatus::Idle, 320.0, true);
        let working = chrome_at_width_with_session(AgentStatus::Working, 320.0, true);

        assert!(session_badge_rect(&idle).is_none());
        // Without the badge, the title falls back to the working-slot math.
        assert!(approx_eq(title_right_boundary(&idle), badges_left_boundary(&idle)));
        assert!(approx_eq(
            title_right_boundary(&working),
            badges_left_boundary(&working) - working_indicator_reserve()
        ));
    }

    #[test]
    fn session_badge_shows_on_wide_titlebars_but_hides_on_narrow_ones() {
        let wide = test_chrome_with_session(AgentStatus::Idle, Some("session-42"));
        let Some(wide_badge) = session_badge_rect(&wide) else {
            panic!("expected badge on wide titlebar");
        };

        assert!(wide_badge.min.x >= wide.titlebar_rect.min.x + SESSION_BADGE_MIN_TITLE_SPACE);
        assert!(session_badge_rect(&chrome_at_width_with_session(AgentStatus::Idle, 320.0, true)).is_none());
        // Without the attention badge the 320 px titlebar still fits the badge.
        assert!(session_badge_rect(&chrome_at_width_with_session(AgentStatus::Idle, 320.0, false)).is_some());
    }

    #[test]
    fn session_badge_protects_the_accent_dot_at_intermediate_widths() {
        // 372 px titlebar with history meter and a short attention badge:
        // the badge slot lands 12 px from the panel edge, where the focused
        // accent dot (centered at 14, radius 5) still extends to 19 px.
        let overlapping = chrome_at_width_with_session(AgentStatus::Idle, 372.0, true);
        assert!(session_badge_rect(&overlapping).is_none());

        // Same width without attention: the badge fits with real title room.
        let without_attention = chrome_at_width_with_session(AgentStatus::Idle, 372.0, false);
        let rect = session_badge_rect(&without_attention).expect("badge fits at 372 px");
        assert!(rect.min.x >= without_attention.titlebar_rect.min.x + 26.0 + SESSION_BADGE_MIN_TITLE_SPACE);
    }

    #[test]
    fn rename_editor_stops_at_painted_badge_or_full_width_when_hidden() {
        // Wide titlebar: the badge fits and the editor stops at its left edge.
        let wide = test_chrome_with_session(AgentStatus::Idle, Some("session-42"));
        let badge = session_badge_rect(&wide).expect("badge on wide titlebar");
        let editor_wide = panel_title_content_rect(&wide);
        assert!(approx_eq(editor_wide.right(), badge.min.x));

        // Narrow titlebar with attention: the badge is hidden and the editor
        // keeps the full titlebar width (down to the badge cluster).
        let narrow = chrome_at_width_with_session(AgentStatus::Idle, 320.0, true);
        assert!(session_badge_rect(&narrow).is_none());
        let editor_narrow = panel_title_content_rect(&narrow);
        assert!(approx_eq(editor_narrow.right(), badges_left_boundary(&narrow)));

        // Without a session at all the editor always keeps the full width.
        let plain = test_chrome(AgentStatus::Idle);
        let editor_plain = panel_title_content_rect(&plain);
        assert!(approx_eq(editor_plain.right(), badges_left_boundary(&plain)));
    }

    #[test]
    fn session_badge_skipped_when_attention_badge_would_cover_it() {
        let session = Rect::from_min_max(Pos2::new(100.0, 30.0), Pos2::new(164.0, 48.0));

        // No attention badge: always shown.
        assert!(session_badge_clears_attention_badge(session, f32::INFINITY));
        // Attention starts right of the session badge: shown.
        assert!(session_badge_clears_attention_badge(session, 164.0));
        assert!(session_badge_clears_attention_badge(session, 200.0));
        // Wide attention summary extends over the session badge: hidden.
        assert!(!session_badge_clears_attention_badge(session, 150.0));
        assert!(!session_badge_clears_attention_badge(session, 90.0));
    }

    #[test]
    fn working_indicator_reserve_covers_three_dots_and_gaps() {
        // Three dots of radius 2 (width 4) plus two 6px gaps, plus the
        // badge gap (6) and the title gap (8).
        assert!(approx_eq(
            working_indicator_reserve(),
            4.0 * 3.0 + 6.0 * 2.0 + 6.0 + 8.0
        ));
        assert!(working_indicator_reserve() > 0.0);
    }

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
