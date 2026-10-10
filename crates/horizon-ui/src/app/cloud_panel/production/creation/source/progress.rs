//! What a running clone shows: its step, how far the phase under way has come with its pace and
//! time left, and what the whole clone has received and for how long it has run.
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

/// The line under the bar, for the phase under way: how far, what Git received in it and how
/// fast, and its time left at `now`, counted down between Git's lines.
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
            Some(left_now) => format!("{} in this phase", left(left_now)),
            None => "almost done".into(),
        });
    }
    parts.join(" · ")
}

/// The line for the whole clone at `now`: what it received, out of about how much when the
/// host told the repository's size, and how long it has run.
pub(super) fn whole(snapshot: &Snapshot, now: Instant) -> Option<String> {
    let ran = now.saturating_duration_since(snapshot.started?);
    let received = snapshot.received.saturating_add(snapshot.receiving);
    let ran = format!("{} so far", elapsed(ran));
    Some(match (received, snapshot.expected) {
        (0, _) => ran,
        (received, Some(expected)) => format!("Received {} of about {} · {ran}", size(received), size(expected)),
        (received, None) => format!("Received {} · {ran}", size(received)),
    })
}

/// A size the way Git writes it, without false precision.
fn size(bytes: u64) -> String {
    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    match bytes {
        bytes if bytes >= GIB => format!("{}.{} GiB", bytes / GIB, bytes % GIB * 10 / GIB),
        bytes if bytes >= MIB => format!("{} MiB", bytes / MIB),
        bytes if bytes >= KIB => format!("{} KiB", bytes / KIB),
        bytes => format!("{bytes} bytes"),
    }
}

/// How long something ran, to the second.
fn elapsed(ran: Duration) -> String {
    let seconds = ran.as_secs();
    match (seconds / 3600, seconds / 60 % 60, seconds % 60) {
        (0, 0, seconds) => format!("{seconds} s"),
        (0, minutes, seconds) => format!("{minutes} min {seconds} s"),
        (hours, minutes, _) => format!("{hours} h {minutes} min"),
    }
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
    // Both lines count live, between Git's own lines too.
    let now = Instant::now();
    let detail = detail(snapshot, now);
    if !detail.is_empty() {
        ui.label(RichText::new(detail).size(12.5).color(theme::FG_DIM()));
    }
    if let Some(whole) = whole(snapshot, now) {
        ui.label(RichText::new(whole).size(12.5).color(theme::FG_SOFT()));
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));
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
    fn the_detail_joins_what_is_known_and_counts_down_the_phase() {
        let now = Instant::now();
        let mut snapshot = Snapshot::default();
        assert_eq!(detail(&snapshot, now), "");
        snapshot.detail = "12.30 MiB | 4.50 MiB/s".into();
        assert_eq!(detail(&snapshot, now), "12.30 MiB | 4.50 MiB/s");
        snapshot.percent = Some(45);
        snapshot.ends = Some(now + Duration::from_secs(8));
        assert_eq!(
            detail(&snapshot, now),
            "45% · 12.30 MiB | 4.50 MiB/s · about 8 s left in this phase"
        );
        assert_eq!(
            detail(&snapshot, now + Duration::from_secs(3)),
            "45% · 12.30 MiB | 4.50 MiB/s · about 5 s left in this phase",
            "live, between Git's lines"
        );
        assert_eq!(
            detail(&snapshot, now + Duration::from_secs(9)),
            "45% · 12.30 MiB | 4.50 MiB/s · almost done"
        );
    }

    #[test]
    fn the_whole_clone_says_what_arrived_out_of_about_how_much_and_for_how_long() {
        let now = Instant::now();
        let mut snapshot = Snapshot::default();
        assert_eq!(whole(&snapshot, now), None, "before the clone starts");
        snapshot.started = Some(now);
        assert_eq!(
            whole(&snapshot, now + Duration::from_secs(4)).as_deref(),
            Some("4 s so far")
        );
        snapshot.received = 200 << 20;
        snapshot.receiving = 48 << 20;
        assert_eq!(
            whole(&snapshot, now + Duration::from_secs(39)).as_deref(),
            Some("Received 248 MiB · 39 s so far")
        );
        snapshot.expected = Some(314 << 20);
        assert_eq!(
            whole(&snapshot, now + Duration::from_secs(125)).as_deref(),
            Some("Received 248 MiB of about 314 MiB · 2 min 5 s so far")
        );
        assert_eq!(size(1536 << 20), "1.5 GiB");
        assert_eq!(size(900), "900 bytes");
    }
}
