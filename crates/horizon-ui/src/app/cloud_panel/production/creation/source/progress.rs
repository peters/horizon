//! What a running clone shows: its step, how far the step has come, the pace and the time left.
use crate::theme;
use egui::{ProgressBar, RichText, Ui};
use horizon_core::cloud_runtime::{Cancellation, repository::source::Snapshot};
use std::time::{Duration, Instant};

/// The time left in the words a person would use: coarse, and never more exact than it is.
pub(super) fn left(eta: Duration) -> String {
    let seconds = eta.as_secs().max(1);
    if seconds < 60 {
        return format!("about {seconds} s left");
    }
    // Round the whole to minutes first, so that 59 min 50 s becomes an hour and not "0 min".
    let minutes = seconds.div_ceil(60);
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("about {minutes} min left"),
        (hours, 0) => format!("about {hours} h left"),
        (hours, minutes) => format!("about {hours} h {minutes} min left"),
    }
}

/// The line above the bar: which step of the clone this is, and what Git is doing in it.
pub(super) fn title(snapshot: &Snapshot) -> String {
    let phase = match snapshot.phase.as_str() {
        "" => "Connecting",
        phase => phase,
    };
    let resumed = if snapshot.resumed { "Resuming · " } else { "" };
    if snapshot.step == 0 {
        format!("{resumed}{phase}…")
    } else {
        format!("{resumed}Step {} of {} · {phase}", snapshot.step, snapshot.steps)
    }
}

/// The line under the bar: what was received and how fast, then the time left at `now`, for
/// the whole clone when the host told its size and for the step otherwise.
pub(super) fn detail(snapshot: &Snapshot, now: Instant) -> String {
    let mut parts = Vec::new();
    if let Some(percent) = snapshot.percent {
        parts.push(format!("{percent}%"));
    }
    if !snapshot.detail.is_empty() {
        parts.push(snapshot.detail.clone());
    }
    if let Some(ends) = snapshot.ends {
        parts.push(match ends.checked_duration_since(now).filter(|left| !left.is_zero()) {
            Some(left_now) if snapshot.whole => left(left_now),
            Some(left_now) => format!("{} in this step", left(left_now)),
            None => "almost done".into(),
        });
    }
    parts.join(" · ")
}

/// Draws the progress of a clone with a Cancel button that raises `cancel`.
pub(super) fn show(ui: &mut Ui, snapshot: &Snapshot, cancel: &Cancellation) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(title(snapshot)).size(13.5).color(theme::FG_SOFT()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Cancel").clicked() {
                cancel.cancel();
            }
        });
    });
    let bar = ProgressBar::new(snapshot.percent.map_or(0.0, |percent| f32::from(percent) / 100.0))
        .desired_height(8.0)
        .corner_radius(4)
        .fill(theme::ACCENT());
    ui.add(if snapshot.percent.is_some() {
        bar
    } else {
        bar.animate(true)
    });
    // Counted down live, between Git's own lines too.
    let detail = detail(snapshot, Instant::now());
    if !detail.is_empty() {
        ui.label(RichText::new(detail).size(12.5).color(theme::FG_DIM()));
    }
    if snapshot.ends.is_some() {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_left_is_coarse_and_never_zero() {
        assert_eq!(left(Duration::from_millis(200)), "about 1 s left");
        assert_eq!(left(Duration::from_secs(42)), "about 42 s left");
        assert_eq!(left(Duration::from_secs(61)), "about 2 min left");
        assert_eq!(left(Duration::from_mins(70)), "about 1 h 10 min left");
        assert_eq!(
            left(Duration::from_secs(7199)),
            "about 2 h left",
            "the carry is not dropped"
        );
        assert_eq!(left(Duration::from_secs(3599)), "about 1 h left");
    }

    #[test]
    fn the_title_names_the_step_and_a_resumed_clone_says_so() {
        let mut snapshot = Snapshot::default();
        assert_eq!(title(&snapshot), "Connecting…");
        snapshot.step = 2;
        snapshot.steps = 3;
        snapshot.phase = "Receiving objects".into();
        assert_eq!(title(&snapshot), "Step 2 of 3 · Receiving objects");
        snapshot.resumed = true;
        assert_eq!(title(&snapshot), "Resuming · Step 2 of 3 · Receiving objects");
    }

    #[test]
    fn the_detail_joins_what_is_known_and_counts_down() {
        let now = Instant::now();
        let mut snapshot = Snapshot::default();
        assert_eq!(detail(&snapshot, now), "");
        snapshot.detail = "12.30 MiB | 4.50 MiB/s".into();
        assert_eq!(detail(&snapshot, now), "12.30 MiB | 4.50 MiB/s");
        snapshot.percent = Some(45);
        snapshot.ends = Some(now + Duration::from_secs(8));
        assert_eq!(
            detail(&snapshot, now),
            "45% · 12.30 MiB | 4.50 MiB/s · about 8 s left in this step"
        );
        snapshot.whole = true;
        assert_eq!(detail(&snapshot, now), "45% · 12.30 MiB | 4.50 MiB/s · about 8 s left");
        assert_eq!(
            detail(&snapshot, now + Duration::from_secs(3)),
            "45% · 12.30 MiB | 4.50 MiB/s · about 5 s left",
            "live, between Git's lines"
        );
        assert_eq!(
            detail(&snapshot, now + Duration::from_secs(9)),
            "45% · 12.30 MiB | 4.50 MiB/s · almost done"
        );
    }
}
