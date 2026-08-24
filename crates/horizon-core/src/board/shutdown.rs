use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Tracks the progress of an asynchronous terminal shutdown.
///
/// Created by [`crate::Board::begin_async_shutdown`] and polled each frame to
/// decide when the application can safely exit.
pub struct ShutdownProgress {
    started_at: Instant,
    terminal_count: usize,
    completed: Arc<AtomicUsize>,
}

impl ShutdownProgress {
    pub(crate) fn new(terminal_count: usize, completed: Arc<AtomicUsize>) -> Self {
        Self {
            started_at: Instant::now(),
            terminal_count,
            completed,
        }
    }

    #[must_use]
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    #[must_use]
    pub fn terminal_count(&self) -> usize {
        self.terminal_count
    }

    #[must_use]
    pub fn terminals_completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.terminals_completed() >= self.terminal_count
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
