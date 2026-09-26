//! Where the last deployment or reconnection spent its time, on the cloud card.
use crate::theme;
use egui::{Color32, CornerRadius, RichText, Sense, Vec2};
use horizon_core::cloud_runtime::{
    Stage, progress,
    timeline::{Phase, Timeline},
};
use std::time::Duration;

const RIBBON_HEIGHT: f32 = 8.0;
/// Half the ribbon height, so its ends are fully round.
const RIBBON_RADIUS: u8 = 4;
/// Sized so label, duration, share and bar fit the runtime card's width.
const BAR_WIDTH: f32 = 30.0;
/// Phases shorter than this are left out of the rows; the ribbon still includes them.
const SHOWN: Duration = Duration::from_millis(100);

/// The ready summary: total time, a chronological ribbon and the breakdown by phase.
pub(super) fn show(ui: &mut egui::Ui, id: u32, runtime: &super::super::Runtime) {
    if runtime.stage != Some(Stage::Ready) {
        return;
    }
    let Some(state) = runtime.state.as_ref() else {
        return;
    };
    let Some(timeline) = state.timeline.as_ref().filter(|timeline| !timeline.total().is_zero()) else {
        // Records from before timelines keep their single total.
        if let Some(seconds) = state.ready_after_seconds {
            ui.small(format!("Worker ready in {}", progress::duration(Duration::from_secs(seconds))))
                .on_hover_text(
                    "Time for the successful deployment attempt. Application startup and reconnect are measured separately.",
                );
        }
        return;
    };
    let verb = if timeline.reconnected { "Reconnected" } else { "Ready" };
    ui.label(
        RichText::new(format!("{verb} in {}", progress::duration(timeline.total())))
            .size(13.0)
            .color(theme::FG_SOFT()),
    );
    ribbon(ui, timeline);
    egui::CollapsingHeader::new(RichText::new("Where the time went").size(12.0).color(theme::FG_DIM()))
        .id_salt(("cloud-timeline", id))
        .show(ui, |ui| rows(ui, id, timeline));
}

/// Every phase in the order it happened, as one bar across the card.
fn ribbon(ui: &mut egui::Ui, timeline: &Timeline) {
    let total = timeline.total().as_secs_f32().max(f32::EPSILON);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), RIBBON_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(RIBBON_RADIUS), theme::BORDER_SUBTLE());
    let visible = || timeline.spans.iter().filter(|span| span.millis > 0);
    let last = visible().count().saturating_sub(1);
    let mut left = rect.left();
    let mut hovered = None;
    for (index, span) in visible().enumerate() {
        let width = rect.width() * Duration::from_millis(span.millis).as_secs_f32() / total;
        let segment = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(left + width, rect.bottom()));
        // Only the outer ends are rounded, so adjacent phases meet flush.
        let corners = CornerRadius {
            nw: if index == 0 { RIBBON_RADIUS } else { 0 },
            sw: if index == 0 { RIBBON_RADIUS } else { 0 },
            ne: if index == last { RIBBON_RADIUS } else { 0 },
            se: if index == last { RIBBON_RADIUS } else { 0 },
        };
        painter.rect_filled(segment, corners, color(span.phase));
        if response
            .hover_pos()
            .is_some_and(|pointer| segment.x_range().contains(pointer.x))
        {
            hovered = Some(span);
        }
        left += width;
    }
    // A phase can repeat (upload and import alternate); the tooltip describes this segment.
    if let Some(span) = hovered {
        response.on_hover_text_at_pointer(format!(
            "{} · {}\n{}",
            timeline.label(span.phase),
            short(Duration::from_millis(span.millis)),
            timeline.detail(span.phase)
        ));
    }
}

/// Phases largest first, with their share of the total.
fn rows(ui: &mut egui::Ui, id: u32, timeline: &Timeline) {
    let total = timeline.total().as_secs_f32().max(f32::EPSILON);
    egui::Grid::new(("cloud-timeline-rows", id))
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            for (phase, spent) in timeline.phases().into_iter().filter(|(_, spent)| *spent >= SHOWN) {
                let share = spent.as_secs_f32() / total;
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color(phase));
                    ui.label(RichText::new(timeline.label(phase)).size(12.0).color(theme::FG_SOFT()));
                })
                .response
                .on_hover_text(timeline.detail(phase));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(short(spent)).size(11.5).monospace().color(theme::FG()));
                });
                ui.label(
                    RichText::new(percent(share))
                        .size(11.0)
                        .monospace()
                        .color(theme::FG_DIM()),
                );
                bar(ui, share, color(phase));
                ui.end_row();
            }
        });
    if let Some(hint) = hint(timeline) {
        ui.add_space(2.0);
        ui.label(RichText::new(hint).size(11.0).italics().color(theme::FG_DIM()));
    }
}

fn bar(ui: &mut egui::Ui, share: f32, fill: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(BAR_WIDTH, 6.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(3), theme::BORDER_SUBTLE());
    let filled = egui::Rect::from_min_size(rect.min, Vec2::new((rect.width() * share).max(2.0), rect.height()));
    ui.painter().rect_filled(filled, CornerRadius::same(3), fill);
}

/// One hue per part of the path (this computer, provider, worker, source), shaded by step.
fn color(phase: Phase) -> Color32 {
    let (hue, shade) = match phase {
        Phase::Prepare => (theme::ACCENT(), 0.0),
        Phase::Build => (theme::ACCENT(), 0.3),
        Phase::Push => (theme::ACCENT(), 0.5),
        Phase::Provision => (theme::PALETTE_YELLOW(), 0.45),
        Phase::ProviderStart => (theme::PALETTE_YELLOW(), 0.0),
        Phase::WorkerStart => (theme::PALETTE_CYAN(), 0.0),
        Phase::Readiness => (theme::PALETTE_CYAN(), 0.45),
        Phase::SourceUpload => (theme::PALETTE_GREEN(), 0.45),
        Phase::SourceImport => (theme::PALETTE_GREEN(), 0.0),
        Phase::Sessions => (theme::FG_DIM(), 0.0),
    };
    theme::blend(hue, theme::PANEL_BG(), shade)
}

fn short(value: Duration) -> String {
    let whole = (value.as_millis() + 500) / 1000;
    if value < Duration::from_millis(9_950) {
        format!("{:.1}s", value.as_secs_f32())
    } else if whole < 60 {
        format!("{whole}s")
    } else {
        format!("{}m {:02}s", whole / 60, whole % 60)
    }
}

fn percent(share: f32) -> String {
    if share < 0.01 {
        "<1%".into()
    } else {
        format!("{:.0}%", share * 100.0)
    }
}

/// Names the one lever that matters when the image download dominates a new cloud.
fn hint(timeline: &Timeline) -> Option<&'static str> {
    let download = timeline
        .phases()
        .into_iter()
        .find_map(|(phase, spent)| (phase == Phase::ProviderStart).then_some(spent))?;
    (!timeline.reconnected && download.as_secs_f32() >= 0.4 * timeline.total().as_secs_f32())
        .then_some("Most of this was the provider downloading the image. Resuming a stopped cloud skips it.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::timeline::Span;

    fn timeline(reconnected: bool, spans: &[(Phase, u64)]) -> Timeline {
        Timeline {
            reconnected,
            spans: spans.iter().map(|&(phase, millis)| Span { phase, millis }).collect(),
        }
    }

    #[test]
    fn durations_and_shares_read_naturally_at_every_scale() {
        assert_eq!(short(Duration::from_millis(1_300)), "1.3s");
        assert_eq!(short(Duration::from_millis(50_700)), "51s");
        assert_eq!(short(Duration::from_millis(147_900)), "2m 28s");
        assert_eq!(short(Duration::from_millis(9_960)), "10s");
        assert_eq!(percent(0.575), "58%");
        assert_eq!(percent(0.004), "<1%");
    }

    #[test]
    fn the_resume_hint_appears_only_when_the_download_dominates_a_new_cloud() {
        let spans = [
            (Phase::Prepare, 9_800),
            (Phase::ProviderStart, 147_900),
            (Phase::SourceImport, 50_700),
        ];
        assert!(hint(&timeline(false, &spans)).is_some());
        assert!(hint(&timeline(true, &spans)).is_none());
        assert!(
            hint(&timeline(
                false,
                &[(Phase::ProviderStart, 10_000), (Phase::SourceImport, 50_000)]
            ))
            .is_none()
        );
        assert!(hint(&timeline(false, &[(Phase::SourceImport, 50_000)])).is_none());
    }

    #[test]
    fn each_part_of_the_path_has_its_own_color() {
        let colors: std::collections::BTreeSet<_> = [
            Phase::Prepare,
            Phase::ProviderStart,
            Phase::WorkerStart,
            Phase::SourceImport,
            Phase::Sessions,
        ]
        .into_iter()
        .map(|phase| color(phase).to_array())
        .collect();
        assert_eq!(colors.len(), 5);
    }
}
