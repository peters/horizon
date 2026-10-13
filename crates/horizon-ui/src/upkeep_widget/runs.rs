//! When the next run starts, and one bar per hour for the last day of runs.
use egui::{CornerRadius, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use horizon_core::upkeep::time::{self, OffsetDateTime};
use horizon_core::upkeep::{Run, RunOutcome, Status, clock, relative};

use crate::theme;

const HOURS: i64 = 24;
const BAR_GAP: f32 = 3.0;
const BAR_HEIGHT: f32 = 26.0;

/// How far ahead of this computer's clock a start time may be and still count.
const CLOCK_SKEW: time::Duration = time::Duration::minutes(5);

/// The run in progress, if the status shows one: the newest run that ran, when it is still
/// running. A skipped run after it only says that it was still busy, and a start in the
/// future is not believed.
pub(super) fn running(status: &Status, now: OffsetDateTime) -> Option<&Run> {
    status
        .runs
        .iter()
        .rev()
        .filter(|run| run.started_at <= now + CLOCK_SKEW)
        .find(|run| !matches!(run.outcome, RunOutcome::Skipped | RunOutcome::Other))
        .filter(|run| run.outcome == RunOutcome::Running)
}

/// How long after its start a run that still shows as running counts as no longer
/// reported: two schedule periods, and at least three hours.
fn stale_after(status: &Status) -> time::Duration {
    let every = i64::from(status.schedule.every_minutes.max(60));
    time::Duration::minutes((2 * every).max(180))
}

/// A short grace after the planned start before a missed run shows as late.
const LATE_AFTER: time::Duration = time::Duration::minutes(10);

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
    let (headline, color, line) = next_run_text(status, now);
    ui.vertical(|ui| {
        ui.set_width(150.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        super::caption(ui, "Next run");
        ui.label(RichText::new(headline).size(20.0).strong().color(color));
        ui.label(RichText::new(line).size(11.5).color(theme::FG_SOFT()));
    });
}

/// The headline, its color and the line under it.
fn next_run_text(status: &Status, now: OffsetDateTime) -> (String, egui::Color32, String) {
    if let Some(run) = running(status, now) {
        let started = relative(run.started_at, now);
        if now - run.started_at > stale_after(status) {
            (
                "No report".to_string(),
                theme::PALETTE_YELLOW(),
                format!("A run started {started}"),
            )
        } else {
            ("Running".to_string(), theme::ACCENT(), format!("Started {started}"))
        }
    } else if status.schedule.paused {
        (
            "Paused".to_string(),
            theme::PALETTE_YELLOW(),
            "No runs until resumed".to_string(),
        )
    } else if let Some(next) = status.schedule.next_run_at {
        if now - next > LATE_AFTER {
            (
                "Late".to_string(),
                theme::PALETTE_YELLOW(),
                format!("Was due at {}", clock(next)),
            )
        } else {
            (relative(next, now), theme::FG(), format!("Next run at {}", clock(next)))
        }
    } else {
        ("Not scheduled".to_string(), theme::FG_SOFT(), String::new())
    }
}

fn bars(ui: &mut Ui, status: &Status, now: OffsetDateTime) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let this_hour = start_of_hour(now);
        let first = this_hour - time::Duration::hours(HOURS - 1);
        let end = this_hour + time::Duration::hours(1);
        // The same runs as the bars, summed without overflow: the counts are untrusted.
        let merged = status
            .runs
            .iter()
            .filter(|run| run.started_at >= first && run.started_at < end)
            .fold(0_u32, |total, run| total.saturating_add(run.merged));
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
        for slot in 0..HOURS {
            let hour = this_hour - time::Duration::hours(HOURS - 1 - slot);
            let index = f32::from(u8::try_from(slot).unwrap_or(0));
            let x = rect.min.x + index * (bar_width + BAR_GAP);
            let bar = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(bar_width, BAR_HEIGHT));
            let runs: Vec<&Run> = status
                .runs
                .iter()
                .filter(|run| run.started_at >= hour && run.started_at < hour + time::Duration::hours(1))
                .collect();
            let summary = Hour::of(&runs);
            paint_bar(ui, bar, summary);
            ui.interact(bar, ui.id().with(("upkeep-hour", slot)), Sense::hover())
                .on_hover_ui(|ui| {
                    ui.label(tooltip(hour, &runs, summary));
                });
        }
        ui.horizontal(|ui| {
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

/// The start of the hour of `now`, so a run that starts on the hour falls in its own slot.
fn start_of_hour(now: OffsetDateTime) -> OffsetDateTime {
    now.replace_time(time::Time::from_hms(now.hour(), 0, 0).unwrap_or(time::Time::MIDNIGHT))
}

/// What the bar of one hour shows: the most telling outcome of its runs and their merges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hour {
    outcome: RunOutcome,
    merged: u32,
}

impl Hour {
    /// `None` for an hour without runs. A run in progress outranks a failure, a failure
    /// outranks a finished run, and a finished run outranks a skipped or unknown one.
    fn of(runs: &[&Run]) -> Option<Self> {
        let rank = |outcome: RunOutcome| match outcome {
            RunOutcome::Running => 3,
            RunOutcome::Failed => 2,
            RunOutcome::Done => 1,
            RunOutcome::Skipped | RunOutcome::Other => 0,
        };
        let outcome = runs
            .iter()
            .map(|run| run.outcome)
            .max_by_key(|outcome| rank(*outcome))?;
        let merged = runs.iter().fold(0_u32, |total, run| total.saturating_add(run.merged));
        Some(Self { outcome, merged })
    }
}

fn paint_bar(ui: &Ui, bar: Rect, hour: Option<Hour>) {
    let painter = ui.painter();
    let radius = CornerRadius::same(3);
    let Some(hour) = hour else {
        painter.rect_filled(bar, radius, theme::alpha(theme::BORDER_SUBTLE(), 120));
        return;
    };
    match hour.outcome {
        RunOutcome::Skipped | RunOutcome::Other => {
            painter.rect_stroke(
                bar.shrink(0.5),
                radius,
                Stroke::new(1.0, theme::alpha(theme::PALETTE_YELLOW(), 150)),
                StrokeKind::Inside,
            );
        }
        RunOutcome::Failed => {
            painter.rect_filled(bar, radius, theme::alpha(theme::PALETTE_RED(), 150));
        }
        RunOutcome::Running => {
            painter.rect_filled(bar, radius, theme::ACCENT());
        }
        RunOutcome::Done => {
            painter.rect_filled(bar, radius, theme::alpha(theme::PALETTE_GREEN(), 46));
        }
    }
    if hour.merged > 0 && hour.outcome != RunOutcome::Running {
        // The merged part rises with the count, up to four in an hour.
        let share = (f32::from(u8::try_from(hour.merged.min(4)).unwrap_or(4)) / 4.0).max(0.3);
        let top = bar.max.y - bar.height() * share;
        let filled = Rect::from_min_max(Pos2::new(bar.min.x, top), bar.max);
        painter.rect_filled(filled, radius, theme::alpha(theme::PALETTE_GREEN(), 200));
    }
}

fn tooltip(hour: OffsetDateTime, runs: &[&Run], summary: Option<Hour>) -> String {
    let at = clock(hour);
    match (runs, summary) {
        ([], _) | (_, None) => format!("{at}: no run"),
        ([run], _) => format!("{at}: {}", describe(run)),
        (runs, Some(summary)) => format!("{at}: {} runs, {} merged", runs.len(), summary.merged),
    }
}

fn describe(run: &Run) -> String {
    let length = run
        .finished_at
        .map(|end| format!(" after {} min", (end - run.started_at).whole_minutes().max(1)))
        .unwrap_or_default();
    match run.outcome {
        RunOutcome::Done if run.merged == 0 => format!("done{length}, nothing to merge"),
        RunOutcome::Done => format!("done{length}, {} merged", run.merged),
        RunOutcome::Running => "running".to_string(),
        RunOutcome::Failed => format!("failed{length}"),
        RunOutcome::Skipped => "skipped, the previous run was still busy".to_string(),
        RunOutcome::Other => "an outcome this Horizon does not know".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::upkeep::parse_time;

    fn run(started_at: &str, outcome: RunOutcome, merged: u32, finished_at: Option<&str>) -> Run {
        serde_json::from_value(serde_json::json!({
            "started_at": started_at,
            "finished_at": finished_at,
            "outcome": match outcome {
                RunOutcome::Done => "done",
                RunOutcome::Running => "running",
                RunOutcome::Failed => "failed",
                RunOutcome::Skipped => "skipped",
                RunOutcome::Other => "other",
            },
            "merged": merged,
        }))
        .unwrap()
    }

    #[test]
    fn an_hour_with_several_runs_shows_all_their_merges() {
        let first = run(
            "2026-10-13T14:05:00Z",
            RunOutcome::Done,
            0,
            Some("2026-10-13T14:09:00Z"),
        );
        let second = run("2026-10-13T14:40:00Z", RunOutcome::Done, u32::MAX, None);
        let hour = Hour::of(&[&first, &second]).unwrap();
        assert_eq!(
            hour,
            Hour {
                outcome: RunOutcome::Done,
                merged: u32::MAX
            },
            "saturates"
        );
        let at = parse_time("2026-10-13T14:00:00Z").unwrap();
        assert_eq!(
            tooltip(at, &[&first, &second], Some(hour)),
            format!("14:00 UTC: 2 runs, {} merged", u32::MAX)
        );
        assert_eq!(
            tooltip(at, &[&first], Hour::of(&[&first])),
            "14:00 UTC: done after 4 min, nothing to merge"
        );
        let failed = run("2026-10-13T14:40:00Z", RunOutcome::Failed, 0, None);
        assert_eq!(tooltip(at, &[&failed], Hour::of(&[&failed])), "14:00 UTC: failed");
        assert_eq!(Hour::of(&[&first, &failed]).unwrap().outcome, RunOutcome::Failed);
        assert_eq!(Hour::of(&[]), None);
    }

    #[test]
    fn a_status_that_stopped_reporting_does_not_look_live() {
        let now = parse_time("2026-10-13T14:37:00Z").unwrap();
        let status = |runs: &str, next: &str| -> Status {
            serde_json::from_str(&format!(
                r#"{{"schedule":{{"every_minutes":60,"next_run_at":"{next}"}},"runs":[{runs}]}}"#
            ))
            .unwrap()
        };
        let text = |status: &Status| {
            let (headline, _, line) = next_run_text(status, now);
            format!("{headline} / {line}")
        };
        let running = r#"{"started_at":"2026-10-13T14:00:00Z","outcome":"running"}"#;
        assert_eq!(
            text(&status(running, "2026-10-13T15:00:00Z")),
            "Running / Started 37 min ago"
        );
        let stuck = r#"{"started_at":"2026-10-11T14:00:00Z","outcome":"running"}"#;
        assert_eq!(
            text(&status(stuck, "2026-10-11T15:00:00Z")),
            "No report / A run started 2 d ago"
        );
        assert_eq!(
            text(&status("", "2026-10-13T15:00:00Z")),
            "in 23 min / Next run at 15:00 UTC"
        );
        assert_eq!(
            text(&status("", "2026-10-13T14:30:00Z")),
            "7 min ago / Next run at 14:30 UTC",
            "within the grace"
        );
        assert_eq!(text(&status("", "2026-10-13T12:00:00Z")), "Late / Was due at 12:00 UTC");
    }

    #[test]
    fn a_skipped_run_does_not_hide_the_run_that_is_still_busy() {
        let now = parse_time("2026-10-13T15:10:00Z").unwrap();
        let status: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T14:00:00Z","outcome":"running"},
                        {"started_at":"2026-10-13T15:00:00Z","outcome":"skipped"},
                        {"started_at":"2026-10-16T15:00:00Z","outcome":"running"}]}"#,
        )
        .unwrap();
        let run = running(&status, now).unwrap();
        assert_eq!(
            run.started_at,
            parse_time("2026-10-13T14:00:00Z").unwrap(),
            "the future run is not believed"
        );
        let done: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T14:00:00Z","outcome":"running"},
                        {"started_at":"2026-10-13T15:00:00Z","outcome":"done"}]}"#,
        )
        .unwrap();
        assert!(running(&done, now).is_none(), "a later finished run ends an old one");
    }

    #[test]
    fn a_run_on_the_hour_belongs_to_that_hour() {
        let now = parse_time("2026-10-13T14:37:12.345Z").unwrap();
        assert_eq!(start_of_hour(now), parse_time("2026-10-13T14:00:00Z").unwrap());
    }
}
