//! The microphone icon of the title bar of a panel.
//!
//! The icon states what the take of speech input does with one panel: idle, red
//! with a pulsing ring while its recorder runs, and pulsing amber while its take is
//! transcribed. One press starts the take of the panel, and the press of that take
//! ends it. The icon takes the focus of the keyboard, so `Enter` or `Space` presses
//! it once.
//!
//! The words of the icon are built only while the pointer is over it, so an idle
//! title bar lays out no text of the icon in a frame.

use std::time::Instant;

use egui::{Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::PanelId;

use crate::theme;

use super::super::speech::{MicState, PanelMic, clock_text, primary_modifier_label};

/// The side of the square of the icon, on the centre line of the title bar.
const MIC_SIDE: f32 = 16.0;

/// The centre of the icon, from the right edge of the panel.
const MIC_FROM_RIGHT: f32 = 44.0;

/// The room that the hit target of the icon adds on each side.
///
/// The close control keeps its own square beside the icon, so the two targets stay
/// apart: a press on the icon never closes the panel.
const MIC_HIT_PAD: f32 = 4.0;

/// The red of a take that records.
const MIC_RECORDING: Color32 = Color32::from_rgb(232, 72, 72);

/// The square of the icon, on the centre line of the title bar of the panel.
pub(in crate::app) fn mic_rect(panel_rect: Rect, titlebar_rect: Rect) -> Rect {
    Rect::from_center_size(
        Pos2::new(panel_rect.max.x - MIC_FROM_RIGHT, titlebar_rect.center().y),
        Vec2::splat(MIC_SIDE),
    )
}

/// What the icon of one panel shows, for the paint of the title bar.
#[derive(Clone, Copy)]
pub(in crate::app) struct MicControl {
    pub(in crate::app) rect: Rect,
    pub(in crate::app) hovered: bool,
    pub(in crate::app) state: MicState,
}

/// The widget of the icon of one frame, and what the title bar paints of it.
pub(super) struct MicWidget {
    pub response: egui::Response,
    pub control: MicControl,
}

/// The accessibility words of one state of the icon.
pub(super) const fn mic_accessibility_label(state: MicState) -> &'static str {
    match state {
        MicState::Idle => "Start dictation",
        MicState::Recording => "Stop dictation; recording",
        MicState::Busy => "Dictation transcription in progress",
    }
}

/// The accessibility information of the icon: the words of its state, and the clock
/// of a take that records as its value.
fn mic_widget_info(state: MicState, enabled: bool, clock: Option<&str>) -> egui::WidgetInfo {
    let mut info = egui::WidgetInfo::labeled(egui::Role::Button, enabled, mic_accessibility_label(state));
    // The accessible value is the only place where a person who does not see the
    // icon reads how long the take runs.
    info.current_text_value = clock.map(str::to_owned);
    info
}

/// The widget of the icon: a press of the pointer, or `Enter` or `Space` while it
/// has the focus.
fn mic_control_response(
    ui: &egui::Ui,
    rect: Rect,
    id: Id,
    enabled: bool,
    state: MicState,
    clock: Option<&str>,
) -> egui::Response {
    let response = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    response.widget_info(|| mic_widget_info(state, enabled, clock));
    response
}

/// Show the icon of `panel` in `rect`, and answer the widget of this frame.
pub(super) fn show(ui: &mut egui::Ui, panel_id: PanelId, rect: Rect, mic: PanelMic) -> MicWidget {
    let enabled = mic.enabled && ui.is_enabled();
    let clock = take_clock(&mic);
    let response = mic_control_response(
        ui,
        rect.expand2(Vec2::splat(MIC_HIT_PAD)),
        ui.make_persistent_id(("panel_mic", panel_id.0)),
        enabled,
        mic.state,
        clock.as_deref(),
    );
    // The sentence is laid out only while the pointer is over the icon, so a title
    // bar of a frame without a hovered icon builds no text.
    let response = response.on_hover_ui(|ui| {
        ui.set_max_width(crate::text::stable_tooltip_max_width(ui));
        ui.label(hover_text(&mic));
    });
    let control = MicControl {
        rect,
        hovered: enabled && (response.hovered() || response.has_focus()),
        state: mic.state,
    };
    MicWidget { response, control }
}

/// The clock of the take of this panel, while it records.
fn take_clock(mic: &PanelMic) -> Option<String> {
    let started = mic.started?;
    (mic.state == MicState::Recording).then(|| clock_text(Instant::now().saturating_duration_since(started)))
}

/// The sentence of the momentary tooltip of the icon.
fn hover_text(mic: &PanelMic) -> String {
    match mic.state {
        MicState::Recording => match take_clock(mic) {
            Some(clock) => format!("Recording into this panel (click to stop): {clock}"),
            None => "Recording into this panel (click to stop)".to_owned(),
        },
        MicState::Busy => "Transcribing dictation for this panel".to_owned(),
        MicState::Idle if mic.active => "Dictation is active in another panel".to_owned(),
        MicState::Idle => match mic.hotkey {
            Some(hotkey) => format!(
                "Dictate into this panel (click, or hold: {})",
                hotkey.display_label(primary_modifier_label())
            ),
            None => "Dictate into this panel (click to start/stop)".to_owned(),
        },
    }
}

/// Paint the icon: a capsule body, a cradle of nine points, and a stem.
///
/// A take that records is red with a pulsing ring, and a take that is transcribed
/// pulses amber. Both need frames while they run, and no other state does.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the pulse alpha stays within [0, 255]"
)]
pub(in crate::app) fn paint(ui: &egui::Ui, painter: &egui::Painter, mic: MicControl) {
    // Cradle points of a radius of 4.5 points, in steps of 22.5°, so the hot path of
    // the title bar allocates nothing in a frame.
    const CRADLE: [Vec2; 9] = [
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
            painter.circle_stroke(
                center,
                9.0 + pulse * 2.0,
                Stroke::new(1.2_f32, theme::alpha(MIC_RECORDING, 90 + (pulse * 120.0) as u8)),
            );
            MIC_RECORDING
        }
        MicState::Busy => theme::alpha(theme::PALETTE_YELLOW(), 140 + (pulse * 100.0) as u8),
    };

    painter.rect_filled(
        Rect::from_center_size(center + Vec2::new(0.0, -2.5), Vec2::new(5.0, 8.0)),
        CornerRadius::same(3),
        color,
    );
    let cradle_center = center + Vec2::new(0.0, 0.5);
    let cradle_stroke = Stroke::new(1.3_f32, color);
    for pair in CRADLE.windows(2) {
        painter.line_segment([cradle_center + pair[0], cradle_center + pair[1]], cradle_stroke);
    }
    painter.line_segment(
        [center + Vec2::new(0.0, 5.0), center + Vec2::new(0.0, 7.0)],
        Stroke::new(1.3_f32, color),
    );
}

#[cfg(test)]
mod tests;
