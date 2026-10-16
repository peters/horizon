//! The daily maintenance pass: how its last run ended and the lines its agent wrote.
use egui::{Color32, CornerRadius, Sense, Ui, Vec2};
use horizon_core::upkeep::time::OffsetDateTime;
use horizon_core::upkeep::{Run, RunKind, RunOutcome, Status, clock};

use crate::theme;

pub(super) fn show(ui: &mut Ui, status: &Status, now: OffsetDateTime) {
    let last = last_pass(status, now);
    let has_lines = status.activity.iter().any(|entry| entry.daily);
    if last.is_none() && !has_lines {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // The mark that the bars of the last 24 hours show under the hour of a pass.
        let (mark, _) = ui.allocate_exact_size(Vec2::new(10.0, 3.0), Sense::hover());
        ui.painter()
            .rect_filled(mark, CornerRadius::same(1), theme::PALETTE_CYAN());
        super::caption(ui, "Daily maintenance");
        if let Some(run) = last {
            let (text, color) = summary(status, run, now);
            super::pill(ui, &text, color);
        }
    });
    if has_lines {
        super::lines(ui, status, true);
    }
}

/// The newest daily pass that started, by start time. A start in the future is not believed.
fn last_pass(status: &Status, now: OffsetDateTime) -> Option<&Run> {
    status
        .runs
        .iter()
        .filter(|run| run.kind == RunKind::Daily && run.started_at <= now + super::CLOCK_SKEW)
        .max_by_key(|run| run.started_at)
}

/// The pill of the last pass. A pass that still shows as running long after its time limit
/// no longer reports, as the next run card says too.
fn summary(status: &Status, run: &Run, now: OffsetDateTime) -> (String, Color32) {
    let at = clock(run.started_at);
    let length = run
        .finished_at
        .map(|end| format!(", {} min", (end - run.started_at).whole_minutes().max(1)))
        .unwrap_or_default();
    match run.outcome {
        RunOutcome::Running if now - run.started_at > super::runs::stale_after(status, run) => {
            (format!("No report since {at}"), theme::PALETTE_YELLOW())
        }
        RunOutcome::Running => (format!("Running since {at}"), theme::ACCENT()),
        RunOutcome::Done => (format!("Done {at}{length}"), theme::PALETTE_GREEN()),
        RunOutcome::Failed => (format!("Failed {at}{length}"), theme::PALETTE_RED()),
        RunOutcome::Skipped | RunOutcome::Other => (at, theme::FG_DIM()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::upkeep::parse_time;

    #[test]
    fn the_section_shows_the_newest_pass_that_started() {
        let status: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T00:05:00Z","finished_at":"2026-10-13T00:39:00Z","outcome":"done","kind":"daily"},
                        {"started_at":"2026-10-13T01:00:00Z","outcome":"done"},
                        {"started_at":"2026-10-12T00:05:00Z","outcome":"failed","kind":"daily"},
                        {"started_at":"2027-01-01T00:00:00Z","outcome":"running","kind":"daily"}]}"#,
        )
        .unwrap();
        let now = parse_time("2026-10-13T09:00:00Z").unwrap();
        let run = last_pass(&status, now).unwrap();
        assert_eq!(summary(&status, run, now).0, "Done 00:05 UTC, 34 min");
        let stuck: Status = serde_json::from_str(
            r#"{"runs":[{"started_at":"2026-10-13T00:05:00Z","outcome":"running","kind":"daily"}]}"#,
        )
        .unwrap();
        let run = last_pass(&stuck, now).unwrap();
        assert_eq!(summary(&stuck, run, now).0, "No report since 00:05 UTC");
        let early = parse_time("2026-10-13T01:00:00Z").unwrap();
        assert_eq!(summary(&stuck, run, early).0, "Running since 00:05 UTC");
        let none: Status = serde_json::from_str(r#"{"runs":[{"started_at":"2026-10-13T01:00:00Z"}]}"#).unwrap();
        assert!(last_pass(&none, now).is_none());
    }
}
