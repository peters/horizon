use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

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
}
