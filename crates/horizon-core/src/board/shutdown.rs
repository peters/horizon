use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Tracks the progress of an asynchronous panel shutdown.
///
/// Created by [`crate::Board::begin_async_shutdown`] and polled each frame to
/// decide when the application can safely exit.
pub struct ShutdownProgress {
    started_at: Instant,
    panel_count: usize,
    terminal_joins_completed: Arc<AtomicUsize>,
    browser_count: usize,
    browser_shutdown_signals: Mutex<Vec<mpsc::Receiver<()>>>,
    browsers_completed: AtomicUsize,
}

impl ShutdownProgress {
    pub(crate) fn new(
        panel_count: usize,
        terminal_joins_completed: Arc<AtomicUsize>,
        browser_shutdown_signals: Vec<mpsc::Receiver<()>>,
    ) -> Self {
        let browser_count = browser_shutdown_signals.len();
        Self {
            started_at: Instant::now(),
            panel_count,
            terminal_joins_completed,
            browser_count,
            browser_shutdown_signals: Mutex::new(browser_shutdown_signals),
            browsers_completed: AtomicUsize::new(0),
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
        self.poll_browser_shutdown_signals();
        self.terminal_joins_completed.load(Ordering::Relaxed) + self.browsers_completed.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.panels_completed() >= self.panel_count
    }

    /// Whether every browser driver has released its Chrome process and
    /// profile lock. Terminal joins may safely remain detached after this.
    #[must_use]
    pub fn browser_shutdown_is_complete(&self) -> bool {
        self.poll_browser_shutdown_signals();
        self.browsers_completed.load(Ordering::Relaxed) >= self.browser_count
    }

    fn poll_browser_shutdown_signals(&self) {
        let mut signals = self
            .browser_shutdown_signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        signals.retain(|signal| match signal.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                self.browsers_completed.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
        });
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
        let progress = ShutdownProgress::new(1, Arc::clone(&completed), Vec::new());
        let worker = std::thread::spawn(move || {
            completed.fetch_add(1, Ordering::Relaxed);
        });

        assert!(progress.wait_for_completion(Duration::from_secs(1)));
        assert!(worker.join().is_ok());
    }

    #[test]
    fn completion_wait_honors_timeout() {
        let progress = ShutdownProgress::new(1, Arc::new(AtomicUsize::new(0)), Vec::new());

        assert!(!progress.wait_for_completion(Duration::ZERO));
    }

    #[test]
    fn browser_completion_is_tracked_separately_from_terminal_joins() {
        let (completed_tx, completed_rx) = mpsc::channel();
        let progress = ShutdownProgress::new(2, Arc::new(AtomicUsize::new(0)), vec![completed_rx]);

        assert!(!progress.browser_shutdown_is_complete());
        assert!(completed_tx.send(()).is_ok());
        assert!(progress.browser_shutdown_is_complete());
        assert!(!progress.is_complete());
    }
}
