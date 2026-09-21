//! Measured deployment activity. Unknown totals never produce an invented ETA.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    #[default]
    Bytes,
    Steps,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub detail: String,
    pub observed_at: Instant,
    pub completed: u64,
    pub total: Option<u64>,
    pub unit: Unit,
    /// Bytes actually reported as transferred; cached layers do not inflate speed.
    pub transferred: Option<u64>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            detail: String::new(),
            observed_at: Instant::now(),
            completed: 0,
            total: None,
            unit: Unit::default(),
            transferred: None,
        }
    }
}

impl Progress {
    #[must_use]
    pub fn activity(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            ..Self::default()
        }
    }
}

#[derive(Default)]
pub struct Rate {
    samples: VecDeque<(Duration, u64)>,
}

impl Rate {
    pub fn observe(&mut self, elapsed: Duration, transferred: u64) {
        if self
            .samples
            .back()
            .is_some_and(|&(time, bytes)| elapsed < time || transferred < bytes)
        {
            self.samples.clear();
        }
        if self.samples.back().is_none_or(|&(time, _)| time != elapsed) {
            self.samples.push_back((elapsed, transferred));
        }
        while self.samples.len() > 2
            && self
                .samples
                .front()
                .is_some_and(|&(time, _)| elapsed.saturating_sub(time) > Duration::from_secs(10))
        {
            self.samples.pop_front();
        }
    }

    #[must_use]
    pub fn bytes_per_second(&self) -> Option<u64> {
        let &(first_time, first_bytes) = self.samples.front()?;
        let &(last_time, last_bytes) = self.samples.back()?;
        let millis = last_time.saturating_sub(first_time).as_millis();
        if millis < 1000 || last_bytes <= first_bytes {
            return None;
        }
        u64::try_from(u128::from(last_bytes - first_bytes) * 1000 / millis)
            .ok()
            .filter(|speed| *speed > 0)
    }

    #[must_use]
    pub fn remaining(&self, progress: &Progress) -> Option<Duration> {
        let remaining = progress.total?.checked_sub(progress.completed)?;
        if remaining == 0 || progress.unit != Unit::Bytes {
            return None;
        }
        Some(Duration::from_secs(remaining.div_ceil(self.bytes_per_second()?)))
    }
}

#[must_use]
pub fn duration(value: Duration) -> String {
    let seconds = value.as_secs();
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

#[must_use]
pub fn bytes(value: u64) -> String {
    for (unit, scale) in [("GB", 1_000_000_000), ("MB", 1_000_000), ("kB", 1000)] {
        if value >= scale {
            return format!("{}.{:01} {unit}", value / scale, value % scale * 10 / scale);
        }
    }
    format!("{value} B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_requires_measured_rate_and_a_known_remaining_total() {
        let mut rate = Rate::default();
        rate.observe(Duration::ZERO, 0);
        rate.observe(Duration::from_secs(2), 200);
        let mut progress = Progress {
            completed: 200,
            total: Some(1000),
            ..Progress::default()
        };
        assert_eq!(rate.bytes_per_second(), Some(100));
        assert_eq!(rate.remaining(&progress), Some(Duration::from_secs(8)));
        progress.total = None;
        assert_eq!(rate.remaining(&progress), None);
        rate.observe(Duration::from_secs(3), 10);
        assert_eq!(rate.bytes_per_second(), None, "retry resets the speed baseline");
    }

    #[test]
    fn stalled_transfers_expire_the_old_speed() {
        let mut rate = Rate::default();
        for second in 0..30 {
            rate.observe(Duration::from_secs(second), second.min(5) * 100);
        }
        assert_eq!(rate.bytes_per_second(), None);
    }
}
