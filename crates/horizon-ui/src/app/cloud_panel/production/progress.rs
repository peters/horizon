//! Presentation timing for the current deployment attempt.
use horizon_core::cloud_runtime::{
    Stage,
    progress::{self, Progress, Rate, Unit},
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Timeline {
    started: Option<Instant>,
    active: Option<(Stage, Instant)>,
    finished: Vec<(Stage, Duration)>,
    detail: Option<Progress>,
    rate: Rate,
}

impl Timeline {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn stage(&mut self, stage: Stage, observed_at: Instant) {
        if self.active.is_some_and(|(current, _)| current == stage) {
            return;
        }
        self.finish(observed_at);
        self.started.get_or_insert(observed_at);
        self.detail = None;
        self.rate = Rate::default();
        if !matches!(stage, Stage::Ready | Stage::Deleted | Stage::Stopped) {
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
}
