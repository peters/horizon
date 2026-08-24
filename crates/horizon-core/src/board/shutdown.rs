use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Tracks the progress of an asynchronous panel shutdown.
///
/// Created by [`crate::Board::begin_async_shutdown`] and polled each frame to
/// decide when the application can safely exit.
pub struct ShutdownProgress {
    started_at: Instant,
    panel_count: usize,
    completed: Arc<AtomicUsize>,
}

impl ShutdownProgress {
    pub(crate) fn new(panel_count: usize, completed: Arc<AtomicUsize>) -> Self {
        Self {
            started_at: Instant::now(),
            panel_count,
            completed,
        }
    }

    #[must_use]
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Start a fresh outer timeout window while retaining the same in-flight
    /// teardown counters (for example, when an interrupted session switch is
    /// followed by application exit).
    pub fn restart_timeout_window(&mut self) {
        self.started_at = Instant::now();
    }

    #[must_use]
    pub fn panel_count(&self) -> usize {
        self.panel_count
    }

    #[must_use]
    pub fn panels_completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.panels_completed() >= self.panel_count
    }

    /// Block until every asynchronous panel teardown finishes or the timeout
    /// expires. This is the synchronous fallback for platform exit callbacks
    /// that can bypass the normal frame-by-frame shutdown poll.
    #[must_use]
    pub fn wait_for_completion(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.is_complete() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.is_complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_wait_observes_async_teardown() {
        let completed = Arc::new(AtomicUsize::new(0));
        let progress = ShutdownProgress::new(1, Arc::clone(&completed));
        let worker = std::thread::spawn(move || {
            completed.fetch_add(1, Ordering::Relaxed);
        });

        assert!(progress.wait_for_completion(Duration::from_secs(1)));
        assert!(worker.join().is_ok());
    }

    #[test]
    fn completion_wait_honors_timeout() {
        let progress = ShutdownProgress::new(1, Arc::new(AtomicUsize::new(0)));

        assert!(!progress.wait_for_completion(Duration::ZERO));
    }
}
