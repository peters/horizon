//! Presentation timing for the current deployment or deletion attempt.
use horizon_core::cloud_runtime::{
    Stage,
    progress::{self, Progress, Rate, Unit},
};
use std::time::{Duration, Instant};

/// A step's reported progress and what can be derived from it.
pub(super) struct Measured<'a> {
    pub detail: &'a str,
    pub completed: u64,
    pub total: Option<u64>,
    pub unit: Unit,
    /// Set only when bytes were reported as sent; an activity alone moves nothing.
    pub transferred: Option<u64>,
    pub bytes_per_second: Option<u64>,
    pub remaining: Option<Duration>,
    pub estimable: bool,
}

impl Measured<'_> {
    /// Share done, when a total is known.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total?;
        let permille = u128::from(self.completed.min(total)) * 1000 / u128::from(total);
        Some(f32::from(u16::try_from(permille).unwrap_or(1000)) / 1000.0)
    }

    /// "612 MB / 1.84 GB · 41 MB/s" or "24/26 steps".
    pub fn numbers(&self) -> String {
        let amount = |value| match self.unit {
            Unit::Bytes => progress::bytes(value),
            Unit::Steps => value.to_string(),
        };
        let mut text = match (self.unit, self.total) {
            (Unit::Steps, Some(total)) => format!("{}/{total} steps", self.completed),
            (_, Some(total)) => format!("{} / {}", amount(self.completed), amount(total)),
            (Unit::Bytes, None) if self.transferred.is_some() => format!("{} transferred", amount(self.completed)),
            (_, None) => String::new(),
        };
        if let Some(speed) = self.bytes_per_second {
            text.push_str(" · ");
            text.push_str(&progress::bytes(speed));
            text.push_str("/s");
        }
        text
    }

    /// "~40s left", or what is missing for an estimate.
    pub fn eta(&self) -> Option<String> {
        if let Some(remaining) = self.remaining {
            Some(format!("~{} left", progress::duration(remaining)))
        } else {
            self.estimable.then(|| "ETA once measurable".to_owned())
        }
    }
}

#[derive(Default)]
pub(super) struct Timeline {
    started: Option<Instant>,
    active: Option<(Stage, Instant)>,
    finished: Vec<(Stage, Duration)>,
    outcome: Option<Stage>,
    detail: Option<Progress>,
    rate: Rate,
    /// Set when a deletion starts, so a deletion that fails before its first step
    /// still presents as one.
    deletion: bool,
    /// Which operation this is; output lines carry it so a failure is read from its own.
    attempt: u64,
}

impl Timeline {
    pub fn reset(&mut self) {
        *self = Self {
            attempt: self.attempt + 1,
            ..Self::default()
        };
    }

    pub fn begin_deletion(&mut self) {
        *self = Self {
            deletion: true,
            attempt: self.attempt + 1,
            ..Self::default()
        };
    }

    /// Advances with every reset, so each operation's output is told apart.
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    pub fn stage(&mut self, stage: Stage, observed_at: Instant) {
        if self.active.is_some_and(|(current, _)| current == stage) {
            return;
        }
        self.finish(observed_at);
        self.started.get_or_insert(observed_at);
        self.detail = None;
        self.rate = Rate::default();
        if matches!(stage, Stage::Ready | Stage::Deleted | Stage::Stopped) {
            self.outcome = Some(stage);
        } else {
            self.outcome = None;
            self.active = Some((stage, observed_at));
        }
    }

    pub fn update(&mut self, detail: Progress) {
        if let Some(bytes) = detail.transferred {
            self.rate.observe(
                self.started.map_or(Duration::ZERO, |start| {
                    detail.observed_at.saturating_duration_since(start)
                }),
                bytes,
            );
        } else {
            self.rate = Rate::default();
        }
        self.detail = Some(detail);
    }

    pub fn finish(&mut self, observed_at: Instant) {
        if let Some((stage, start)) = self.active.take() {
            self.finished
                .push((stage, observed_at.saturating_duration_since(start)));
        }
    }

    pub fn stage_label(&self, stage: Stage) -> String {
        if let Some((_, start)) = self.active.filter(|(current, _)| *current == stage) {
            format!("{} · {}", stage.label(), progress::duration(start.elapsed()))
        } else if let Some((_, elapsed)) = self.finished.iter().rev().find(|(current, _)| *current == stage) {
            format!("{} · {}", stage.label(), progress::duration(*elapsed))
        } else {
            stage.label().into()
        }
    }

    /// Stages are contiguous, so a finished attempt took the sum of its stage durations.
    pub fn elapsed(&self) -> Option<Duration> {
        let started = self.started?;
        Some(if self.active.is_some() {
            started.elapsed()
        } else {
            self.finished.iter().map(|(_, duration)| *duration).sum()
        })
    }

    /// Total time of an attempt observed to end in `stage`, such as a completed deletion.
    pub fn ended_in(&self, stage: Stage) -> Option<Duration> {
        if self.outcome == Some(stage) {
            self.elapsed()
        } else {
            None
        }
    }

    /// How long `stage` ran in this attempt: live while it runs, frozen once finished.
    pub fn stage_duration(&self, stage: Stage) -> Option<Duration> {
        if let Some((_, start)) = self.active.filter(|(current, _)| *current == stage) {
            return Some(start.elapsed());
        }
        self.finished
            .iter()
            .rev()
            .find(|(current, _)| *current == stage)
            .map(|(_, elapsed)| *elapsed)
    }

    /// The step running now, else the last one this attempt reported.
    pub fn last_stage(&self) -> Option<Stage> {
        self.active
            .map(|(stage, _)| stage)
            .or_else(|| self.finished.last().map(|(stage, _)| *stage))
    }

    /// The running step's measured progress, for the one-line status.
    pub fn measured(&self) -> Option<Measured<'_>> {
        let detail = self.detail.as_ref()?;
        let running = self.active.is_some();
        let remaining = self.rate.remaining(detail).filter(|_| running);
        Some(Measured {
            detail: &detail.detail,
            completed: detail.completed,
            total: detail.total.filter(|total| *total > 0),
            unit: detail.unit,
            transferred: detail.transferred,
            bytes_per_second: self.rate.bytes_per_second().filter(|_| running),
            remaining,
            // Complete work has nothing left to estimate.
            estimable: detail.total.is_some_and(|total| detail.completed < total),
        })
    }

    pub fn activity(&self) -> Option<&str> {
        self.active
            .and(self.detail.as_ref())
            .map(|detail| detail.detail.as_str())
    }

    /// Whether this attempt is a deletion: begun as one, or it reported a deletion step.
    pub fn is_deletion(&self) -> bool {
        self.deletion
            || self
                .active
                .iter()
                .map(|(stage, _)| stage)
                .chain(self.finished.iter().map(|(stage, _)| stage))
                .any(|stage| Stage::DELETION.contains(stage))
    }

    pub fn render(&self, ui: &mut egui::Ui) {
        let Some(detail) = &self.detail else { return };
        ui.add_space(5.0);
        ui.small(&detail.detail);
        if detail.transferred.is_some() || detail.total.is_some() {
            let amount = match detail.unit {
                Unit::Bytes => progress::bytes(detail.completed),
                Unit::Steps => detail.completed.to_string(),
            };
            let label = detail.total.map_or_else(
                || format!("{amount} transferred"),
                |total| {
                    let total = match detail.unit {
                        Unit::Bytes => progress::bytes(total),
                        Unit::Steps => total.to_string(),
                    };
                    format!("{amount} / {total}")
                },
            );
            if let Some(total) = detail.total.filter(|total| *total > 0) {
                ui.add(
                    egui::ProgressBar::new(
                        f32::from(
                            u16::try_from(u128::from(detail.completed.min(total)) * 1000 / u128::from(total))
                                .unwrap_or_default(),
                        ) / 1000.0,
                    )
                    .text(label),
                );
            } else {
                ui.small(label);
            }
        }
        if self.active.is_none() {
            return;
        }
        if let Some(speed) = self.rate.bytes_per_second() {
            ui.small(format!("{}/s", progress::bytes(speed)));
        }
        if let Some(remaining) = self.rate.remaining(detail) {
            ui.small(format!("ETA ~{}", progress::duration(remaining)));
        } else if detail.total.is_none_or(|total| detail.completed < total) {
            ui.small("ETA unavailable until progress is measurable");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_ui_delivery_preserves_producer_rate_and_stage_duration() {
        let start = Instant::now().checked_sub(Duration::from_secs(30)).unwrap();
        let mut timeline = Timeline::default();
        timeline.stage(Stage::Push, start);
        for (seconds, bytes) in [(0, 0), (2, 200)] {
            timeline.update(Progress {
                observed_at: start + Duration::from_secs(seconds),
                transferred: Some(bytes),
                completed: bytes,
                total: Some(1000),
                ..Progress::default()
            });
        }
        assert_eq!(timeline.rate.bytes_per_second(), Some(100));
        timeline.stage(Stage::Provision, start + Duration::from_secs(3));
        assert_eq!(timeline.stage_label(Stage::Push), "Push image · 0m 03s");
    }
    #[test]
    fn preceding_stages_do_not_inflate_transfer_rate_or_eta() {
        let start = Instant::now();
        let mut timeline = Timeline::default();
        timeline.stage(Stage::Build, start);
        timeline.stage(Stage::Push, start + Duration::from_secs(100));
        for (seconds, bytes) in [(102, 0), (104, 200)] {
            timeline.update(Progress {
                observed_at: start + Duration::from_secs(seconds),
                transferred: Some(bytes),
                completed: bytes,
                total: Some(1000),
                ..Progress::default()
            });
        }
        assert_eq!(timeline.rate.bytes_per_second(), Some(100));
        assert_eq!(
            timeline.rate.remaining(timeline.detail.as_ref().unwrap()),
            Some(Duration::from_secs(8))
        );
    }
    #[test]
    fn deletion_freezes_step_durations_and_the_total_time() {
        let start = Instant::now().checked_sub(Duration::from_secs(60)).unwrap();
        let mut timeline = Timeline::default();
        assert_eq!(timeline.elapsed(), None);
        timeline.stage(Stage::ReleaseDevices, start);
        timeline.update(Progress::activity("Confirming worker identity"));
        assert_eq!(timeline.activity(), Some("Confirming worker identity"));
        assert!(
            timeline.elapsed().unwrap() >= Duration::from_secs(60),
            "running totals are live"
        );
        timeline.stage(Stage::DeleteWorker, start + Duration::from_secs(3));
        assert_eq!(timeline.activity(), None, "a new step starts without the old detail");
        timeline.stage(Stage::DeleteStorage, start + Duration::from_secs(5));
        assert_eq!(timeline.ended_in(Stage::Deleted), None, "still running");
        timeline.stage(Stage::Deleted, start + Duration::from_secs(12));
        assert_eq!(timeline.elapsed(), Some(Duration::from_secs(12)));
        assert_eq!(timeline.ended_in(Stage::Deleted), Some(Duration::from_secs(12)));
        assert_eq!(timeline.activity(), None);
        assert_eq!(
            Stage::DELETION.map(|stage| timeline.stage_label(stage)),
            [
                "Release hosted devices · 0m 03s",
                "Delete worker · 0m 02s",
                "Delete workspace storage · 0m 07s"
            ]
        );
        assert!(timeline.is_deletion());
        let mut failed = Timeline::default();
        failed.stage(Stage::DeleteWorker, start);
        failed.finish(start + Duration::from_secs(4));
        assert!(failed.is_deletion(), "a failed deletion keeps its steps");
        assert_eq!(failed.elapsed(), Some(Duration::from_secs(4)));
        assert_eq!(
            failed.ended_in(Stage::Deleted),
            None,
            "a failed deletion reports no total"
        );
        let mut early = Timeline::default();
        early.begin_deletion();
        assert!(early.is_deletion(), "a deletion that fails before its first step");
        assert_eq!(early.elapsed(), None);
        early.reset();
        assert!(!early.is_deletion(), "the next deployment starts a fresh timeline");
    }
}
