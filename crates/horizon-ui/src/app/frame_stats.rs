use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The meter reports the frames presented during this trailing window, so it
/// reacts equally fast at 30 Hz and at 240 Hz.
const MEASUREMENT_WINDOW: Duration = Duration::from_secs(1);
/// How long the last measurement stays readable after continuous rendering
/// stops, before the meter shows idle.
const IDLE_HOLD: Duration = Duration::from_secs(1);
/// egui shortens every delayed repaint by one predicted frame (1/60 s) and
/// treats a delay that reaches zero as an immediate repaint. Keep the idle
/// refresh comfortably above that so it never counts as continuous rendering.
const IDLE_REFRESH_MARGIN: Duration = Duration::from_millis(100);
/// Bounds memory on very high refresh rates; 1,024 frames is 1 s at 1 kHz.
const MAX_FRAME_TIMESTAMPS: usize = 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FrameStatsSnapshot {
    pub(super) fps: f32,
    pub(super) frame_time_ms: f32,
    pub(super) slowest_frame_time_ms: f32,
    pub(super) sample_count: usize,
}

/// Measures render throughput, not how often Horizon happens to wake up.
///
/// Horizon repaints reactively and polls terminal output on a timer that backs
/// off to 100, 250, 500 and 1,000 ms while idle. The gap before such a timer
/// frame is time spent waiting, so only frames whose previous pass asked for an
/// immediate repaint (input, animation, streaming output, panning) extend the
/// measured run; any other frame ends it.
#[derive(Clone, Debug, Default)]
pub(super) struct FrameStats {
    run_frame_starts: VecDeque<Instant>,
    run_open: bool,
    last_frame_at: Option<Instant>,
}

impl FrameStats {
    /// `continuous` is whether the previous pass requested an immediate repaint,
    /// which makes the interval since that frame pure render and present time.
    pub(super) fn record_frame(&mut self, now: Instant, continuous: bool) {
        if continuous {
            if !self.run_open {
                self.run_frame_starts.clear();
                self.run_frame_starts.extend(self.last_frame_at);
                self.run_open = true;
            }
            self.run_frame_starts.push_back(now);
            while self.run_frame_starts.len() > MAX_FRAME_TIMESTAMPS
                || self
                    .run_frame_starts
                    .front()
                    .is_some_and(|first| now.saturating_duration_since(*first) > MEASUREMENT_WINDOW)
            {
                self.run_frame_starts.pop_front();
            }
        } else {
            self.run_open = false;
        }

        self.last_frame_at = Some(now);
    }

    /// When the held measurement should give way to idle, so the meter is
    /// refreshed once even if nothing else repaints.
    pub(super) fn idle_refresh_after(&self, now: Instant) -> Option<Duration> {
        if self.run_frame_starts.len() < 2 {
            return None;
        }

        let since_run = now.saturating_duration_since(*self.run_frame_starts.back()?);
        if since_run > IDLE_HOLD {
            return None;
        }
        Some(IDLE_HOLD + IDLE_REFRESH_MARGIN - since_run)
    }

    pub(super) fn snapshot(&self) -> FrameStatsSnapshot {
        let (Some(first), Some(last), Some(last_frame_at)) = (
            self.run_frame_starts.front(),
            self.run_frame_starts.back(),
            self.last_frame_at,
        ) else {
            return FrameStatsSnapshot::default();
        };
        let sample_count = self.run_frame_starts.len() - 1;
        let span = last.saturating_duration_since(*first);
        if sample_count == 0 || span.is_zero() || last_frame_at.saturating_duration_since(*last) > IDLE_HOLD {
            return FrameStatsSnapshot::default();
        }

        let slowest_frame_time = self
            .run_frame_starts
            .iter()
            .zip(self.run_frame_starts.iter().skip(1))
            .map(|(earlier, later)| later.saturating_duration_since(*earlier))
            .max()
            .unwrap_or_default();
        let sample_count_f32 = u16::try_from(sample_count).map_or(f32::from(u16::MAX), f32::from);
        let frame_time_ms = span.as_secs_f32() * 1000.0 / sample_count_f32;

        FrameStatsSnapshot {
            fps: 1000.0 / frame_time_ms,
            frame_time_ms,
            slowest_frame_time_ms: slowest_frame_time.as_secs_f32() * 1000.0,
            sample_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{FrameStats, FrameStatsSnapshot, IDLE_HOLD, MAX_FRAME_TIMESTAMPS};

    /// egui's predicted frame time, subtracted from every delayed repaint.
    const EGUI_PREDICTED_DT: Duration = Duration::from_micros(16_667);

    fn render_continuously(frame_stats: &mut FrameStats, start: Instant, deltas_ms: &[u64]) -> Instant {
        frame_stats.record_frame(start, false);

        let mut timestamp = start;
        for delta_ms in deltas_ms {
            timestamp += Duration::from_millis(*delta_ms);
            frame_stats.record_frame(timestamp, true);
        }

        timestamp
    }

    #[test]
    fn frame_stats_average_continuous_frames() {
        let mut frame_stats = FrameStats::default();
        render_continuously(&mut frame_stats, Instant::now(), &[16, 16, 17, 17]);
        let snapshot = frame_stats.snapshot();

        assert_eq!(snapshot.sample_count, 4);
        assert!((snapshot.frame_time_ms - 16.5).abs() < 0.01);
        assert!((snapshot.fps - (1000.0 / 16.5)).abs() < 0.1);
        assert!((snapshot.slowest_frame_time_ms - 17.0).abs() < 0.01);
    }

    #[test]
    fn frame_stats_measure_a_fixed_time_window_at_any_refresh_rate() {
        for (delta_ms, expected_frames) in [(40_u64, 25_usize), (4, 250)] {
            let mut frame_stats = FrameStats::default();
            let frames = usize::try_from(2_000 / delta_ms).expect("frame count fits");
            render_continuously(&mut frame_stats, Instant::now(), &vec![delta_ms; frames]);
            let snapshot = frame_stats.snapshot();

            assert_eq!(snapshot.sample_count, expected_frames, "{delta_ms} ms frames");
            let expected_ms = Duration::from_millis(delta_ms).as_secs_f32() * 1000.0;
            assert!((snapshot.frame_time_ms - expected_ms).abs() < 0.01);
        }
    }

    #[test]
    fn frame_stats_bound_memory_at_extreme_refresh_rates() {
        let mut frame_stats = FrameStats::default();
        let start = Instant::now();
        frame_stats.record_frame(start, false);
        for frame in 1..=4_000_u32 {
            frame_stats.record_frame(start + Duration::from_micros(100) * frame, true);
        }

        assert_eq!(frame_stats.snapshot().sample_count, MAX_FRAME_TIMESTAMPS - 1);
    }

    #[test]
    fn frame_stats_report_the_slowest_frame_that_the_average_hides() {
        let mut frame_stats = FrameStats::default();
        let mut deltas_ms = vec![16; 30];
        deltas_ms[10] = 120;
        render_continuously(&mut frame_stats, Instant::now(), &deltas_ms);
        let snapshot = frame_stats.snapshot();

        assert!(snapshot.fps > 45.0);
        assert!((snapshot.slowest_frame_time_ms - 120.0).abs() < 0.01);
    }

    #[test]
    fn frame_stats_still_measure_slow_continuous_rendering() {
        let mut frame_stats = FrameStats::default();
        render_continuously(&mut frame_stats, Instant::now(), &[300, 300, 300]);

        let snapshot = frame_stats.snapshot();
        assert_eq!(snapshot.sample_count, 3);
        assert!((snapshot.fps - (1000.0 / 300.0)).abs() < 0.01);
    }

    #[test]
    fn frame_stats_ignore_idle_output_polls() {
        let mut frame_stats = FrameStats::default();
        let mut now = Instant::now();
        frame_stats.record_frame(now, false);

        // Horizon's terminal poll backs off while idle; each wake comes from a timer.
        for poll_ms in [100, 100, 250, 250, 500, 500, 1_000] {
            now += Duration::from_millis(poll_ms) - EGUI_PREDICTED_DT;
            frame_stats.record_frame(now, false);
            assert_eq!(
                frame_stats.snapshot(),
                FrameStatsSnapshot::default(),
                "{poll_ms} ms poll"
            );
            assert_eq!(frame_stats.idle_refresh_after(now), None);
        }
    }

    #[test]
    fn frame_stats_hold_the_last_run_through_polls_then_go_idle() {
        let mut frame_stats = FrameStats::default();
        let start = Instant::now();
        let last = render_continuously(&mut frame_stats, start, &[16; 30]);
        let measured = frame_stats.snapshot();

        let poll = last + Duration::from_millis(100);
        frame_stats.record_frame(poll, false);
        assert_eq!(frame_stats.snapshot(), measured);

        let refresh = frame_stats
            .idle_refresh_after(poll)
            .expect("refresh while the run is held");
        assert!(refresh > EGUI_PREDICTED_DT, "an idle refresh must not become immediate");
        let refreshed_at = poll + refresh - EGUI_PREDICTED_DT;
        assert!(refreshed_at.saturating_duration_since(last) > IDLE_HOLD);

        frame_stats.record_frame(refreshed_at, false);
        assert_eq!(frame_stats.snapshot(), FrameStatsSnapshot::default());
        assert_eq!(frame_stats.idle_refresh_after(refreshed_at), None);
    }

    #[test]
    fn frame_stats_start_a_fresh_run_after_a_pause() {
        let mut frame_stats = FrameStats::default();
        let start = Instant::now();
        let last = render_continuously(&mut frame_stats, start, &[8; 20]);
        frame_stats.record_frame(last + Duration::from_millis(400), false);

        let resumed = last + Duration::from_millis(900);
        frame_stats.record_frame(resumed, false);
        frame_stats.record_frame(resumed + Duration::from_millis(16), true);
        frame_stats.record_frame(resumed + Duration::from_millis(32), true);

        let snapshot = frame_stats.snapshot();
        assert_eq!(snapshot.sample_count, 2);
        assert!((snapshot.frame_time_ms - 16.0).abs() < 0.01);
    }
}
