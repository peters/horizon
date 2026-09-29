//! What a running clone shows: its step, how far the step has come, the pace and the time left.
use crate::theme;
use egui::{ProgressBar, RichText, Ui};
use horizon_core::cloud_runtime::{Cancellation, repository::source::Snapshot};
use std::time::Duration;

/// The time left in the words a person would use: coarse, and never more exact than it is.
pub(super) fn left(eta: Duration) -> String {
    let seconds = eta.as_secs().max(1);
    match seconds {
        0..=59 => format!("about {seconds} s left"),
        60..=3599 => format!("about {} min left", seconds.div_ceil(60)),
        _ => format!(
            "about {} h {} min left",
            seconds / 3600,
            (seconds % 3600).div_ceil(60) % 60
        ),
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

/// The line under the bar: what was received and how fast, then the time left.
pub(super) fn detail(snapshot: &Snapshot) -> String {
    let mut parts = Vec::new();
    if let Some(percent) = snapshot.percent {
        parts.push(format!("{percent}%"));
    }
    if !snapshot.detail.is_empty() {
        parts.push(snapshot.detail.clone());
    }
    if let Some(eta) = snapshot.eta {
        parts.push(left(eta));
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
    let detail = detail(snapshot);
    if !detail.is_empty() {
        ui.label(RichText::new(detail).size(12.5).color(theme::FG_DIM()));
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
    fn the_detail_joins_what_is_known() {
        let mut snapshot = Snapshot::default();
        assert_eq!(detail(&snapshot), "");
        snapshot.detail = "12.30 MiB | 4.50 MiB/s".into();
        assert_eq!(detail(&snapshot), "12.30 MiB | 4.50 MiB/s");
        snapshot.percent = Some(45);
        snapshot.eta = Some(Duration::from_secs(8));
        assert_eq!(detail(&snapshot), "45% · 12.30 MiB | 4.50 MiB/s · about 8 s left");
    }
}
