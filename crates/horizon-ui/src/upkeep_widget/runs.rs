//! When the next run starts, and one bar per hour for the last day of runs.
use egui::{CornerRadius, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_core::upkeep::time::{self, OffsetDateTime};
use horizon_core::upkeep::{Run, RunOutcome, Status, clock, relative};

use crate::theme;

const HOURS: i64 = 24;
const BAR_GAP: f32 = 3.0;
const BAR_HEIGHT: f32 = 26.0;

/// The run in progress, if the status shows one.
pub(super) fn running(status: &Status) -> Option<&Run> {
    status.runs.last().filter(|run| run.outcome == RunOutcome::Running)
}

pub(super) fn show(ui: &mut Ui, status: &Status, now: OffsetDateTime) {
    Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                next_run(ui, status, now);
                bars(ui, status, now);
            });
        });
}

/// "in 22 min" big, with what it means under it.
fn next_run(ui: &mut Ui, status: &Status, now: OffsetDateTime) {
    let (headline, color, line) = if let Some(run) = running(status) {
        let started = relative(run.started_at, now);
        ("Running".to_string(), theme::ACCENT(), format!("Started {started}"))
    } else if status.schedule.paused {
        (
            "Paused".to_string(),
            theme::PALETTE_YELLOW(),
            "No runs until resumed".to_string(),
        )
    } else if let Some(next) = status.schedule.next_run_at {
        (relative(next, now), theme::FG(), format!("Next run at {}", clock(next)))
    } else {
        ("Not scheduled".to_string(), theme::FG_SOFT(), String::new())
    };
    ui.vertical(|ui| {
        ui.set_width(150.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        super::caption(ui, "Next run");
        ui.label(RichText::new(headline).size(20.0).strong().color(color));
        ui.label(RichText::new(line).size(11.5).color(theme::FG_SOFT()));
    });
}

fn bars(ui: &mut Ui, status: &Status, now: OffsetDateTime) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let merged: u32 = status
            .runs
            .iter()
            .filter(|run| now - run.started_at <= time::Duration::hours(HOURS))
            .map(|run| run.merged)
            .sum();
        ui.horizontal(|ui| {
            super::caption(ui, "Last 24 hours");
            ui.label(
                RichText::new(format!("{merged} merged"))
                    .size(11.0)
                    .monospace()
                    .color(theme::FG_SOFT()),
            );
        });
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, BAR_HEIGHT), Sense::hover());
        let count = f32::from(u8::try_from(HOURS).unwrap_or(24));
        let bar_width = ((width - BAR_GAP * (count - 1.0)) / count).max(2.0);
        let this_hour = now.replace_minute(0).and_then(|t| t.replace_second(0)).unwrap_or(now);
        for slot in 0..HOURS {
            let hour = this_hour - time::Duration::hours(HOURS - 1 - slot);
            let index = f32::from(u8::try_from(slot).unwrap_or(0));
            let x = rect.min.x + index * (bar_width + BAR_GAP);
            let bar = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(bar_width, BAR_HEIGHT));
            let run = status
                .runs
                .iter()
                .find(|run| run.started_at >= hour && run.started_at < hour + time::Duration::hours(1));
            paint_bar(ui, bar, run);
            let tip = tooltip(hour, run);
            ui.interact(bar, ui.id().with(("upkeep-hour", slot)), Sense::hover())
                .on_hover_text(tip);
        }
        ui.horizontal(|ui| {
            let first = this_hour - time::Duration::hours(HOURS - 1);
            ui.label(
                RichText::new(clock(first))
                    .size(10.0)
                    .monospace()
                    .color(theme::FG_DIM()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("now").size(10.0).monospace().color(theme::FG_DIM()));
            });
        });
    });
}

fn paint_bar(ui: &Ui, bar: Rect, run: Option<&Run>) {
    let painter = ui.painter();
    let radius = CornerRadius::same(3);
    match run.map(|run| (run.outcome, run.merged)) {
        None => {
            painter.rect_filled(bar, radius, theme::alpha(theme::BORDER_SUBTLE(), 120));
        }
        Some((RunOutcome::Skipped, _)) => {
            painter.rect_stroke(
                bar.shrink(0.5),
                radius,
                Stroke::new(1.0, theme::alpha(theme::PALETTE_YELLOW(), 150)),
                StrokeKind::Inside,
            );
        }
        Some((RunOutcome::Failed, _)) => {
            painter.rect_filled(bar, radius, theme::alpha(theme::PALETTE_RED(), 150));
        }
        Some((RunOutcome::Running, _)) => {
            painter.rect_filled(bar, radius, theme::ACCENT());
        }
        Some((RunOutcome::Done, merged)) => {
            painter.rect_filled(bar, radius, theme::alpha(theme::PALETTE_GREEN(), 46));
            if merged > 0 {
                // The merged part rises with the count, up to four in a run.
                let share = (f32::from(u8::try_from(merged.min(4)).unwrap_or(4)) / 4.0).max(0.3);
                let top = bar.max.y - bar.height() * share;
                let filled = Rect::from_min_max(Pos2::new(bar.min.x, top), bar.max);
                painter.rect_filled(filled, radius, theme::alpha(theme::PALETTE_GREEN(), 200));
            }
        }
    }
}

fn tooltip(hour: OffsetDateTime, run: Option<&Run>) -> String {
    let at = clock(hour);
    let Some(run) = run else {
        return format!("{at}: no run");
    };
    let length = run
        .finished_at
        .map(|end| format!("{} min", (end - run.started_at).whole_minutes().max(1)))
        .unwrap_or_default();
    match run.outcome {
        RunOutcome::Done if run.merged == 0 => format!("{at}: {length}, nothing to merge"),
        RunOutcome::Done => format!("{at}: {length}, {} merged", run.merged),
        RunOutcome::Running => format!("{at}: running"),
        RunOutcome::Failed => format!("{at}: failed after {length}"),
        RunOutcome::Skipped => format!("{at}: skipped, the previous run was still busy"),
    }
}
