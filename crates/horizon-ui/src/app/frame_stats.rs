use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_FRAME_SAMPLES: usize = 120;
const STALE_SAMPLE_CUTOFF: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FrameStatsSnapshot {
    pub(super) fps: f32,
    pub(super) frame_time_ms: f32,
    pub(super) sample_count: usize,
}

#[derive(Clone, Debug)]
pub(super) struct FrameStats {
    last_frame_at: Option<Instant>,
    frame_times_ms: VecDeque<f32>,
    frame_time_sum_ms: f32,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self {
            last_frame_at: None,
            frame_times_ms: VecDeque::with_capacity(MAX_FRAME_SAMPLES),
            frame_time_sum_ms: 0.0,
        }
    }
}

impl FrameStats {
    pub(super) fn record_frame(&mut self, now: Instant) {
        if let Some(last_frame_at) = self.last_frame_at {
            let frame_time = now.saturating_duration_since(last_frame_at);
            if frame_time <= STALE_SAMPLE_CUTOFF {
                self.push_sample(frame_time);
            } else {
                self.clear_samples();
            }
        }

        self.last_frame_at = Some(now);
    }

    pub(super) fn snapshot(&self) -> FrameStatsSnapshot {
        let sample_count = self.frame_times_ms.len();
        if sample_count == 0 {
            return FrameStatsSnapshot::default();
        }

        let sample_count_f32 = u16::try_from(sample_count).map_or(f32::from(u16::MAX), f32::from);
        let frame_time_ms = self.frame_time_sum_ms / sample_count_f32;
        let fps = if frame_time_ms <= f32::EPSILON {
            0.0
        } else {
            1000.0 / frame_time_ms
        };

        FrameStatsSnapshot {
            fps,
            frame_time_ms,
            sample_count,
        }
    }

    fn push_sample(&mut self, frame_time: Duration) {
        let frame_time_ms = frame_time.as_secs_f32() * 1000.0;
        self.frame_times_ms.push_back(frame_time_ms);
        self.frame_time_sum_ms += frame_time_ms;

        while self.frame_times_ms.len() > MAX_FRAME_SAMPLES {
            if let Some(evicted) = self.frame_times_ms.pop_front() {
                self.frame_time_sum_ms -= evicted;
            }
        }
    }

    fn clear_samples(&mut self) {
        self.frame_times_ms.clear();
        self.frame_time_sum_ms = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{FrameStats, FrameStatsSnapshot};

    fn record_samples(frame_stats: &mut FrameStats, deltas_ms: &[u64]) -> FrameStatsSnapshot {
        let start = Instant::now();
        frame_stats.record_frame(start);

        let mut timestamp = start;
        for delta_ms in deltas_ms {
            timestamp += Duration::from_millis(*delta_ms);
            frame_stats.record_frame(timestamp);
        }

        frame_stats.snapshot()
    }

    #[test]
    fn frame_stats_average_recent_frames() {
        let mut frame_stats = FrameStats::default();
        let snapshot = record_samples(&mut frame_stats, &[16, 16, 17, 17]);

        assert_eq!(snapshot.sample_count, 4);
        assert!((snapshot.frame_time_ms - 16.5).abs() < 0.01);
        assert!((snapshot.fps - (1000.0 / 16.5)).abs() < 0.1);
    }

    #[test]
    fn frame_stats_drop_stale_samples_after_idle_gap() {
        let mut frame_stats = FrameStats::default();
        let start = Instant::now();

        frame_stats.record_frame(start);
        frame_stats.record_frame(start + Duration::from_millis(16));
        frame_stats.record_frame(start + Duration::from_millis(32));
        assert_eq!(frame_stats.snapshot().sample_count, 2);

        frame_stats.record_frame(start + Duration::from_secs(2));
        assert_eq!(frame_stats.snapshot(), FrameStatsSnapshot::default());

        frame_stats.record_frame(start + Duration::from_secs(2) + Duration::from_millis(10));
        assert_eq!(frame_stats.snapshot().sample_count, 1);
    }
}
