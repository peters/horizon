use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::browser::BrowserShutdownSignal;

pub(super) const FORCED_BROWSER_SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

const FORCE_NOT_STARTED: usize = 0;
const FORCE_RUNNING: usize = 1;
const FORCE_SUCCEEDED: usize = 2;
const FORCE_FAILED: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcedBrowserShutdownStatus {
    NotStarted,
    Running,
    Succeeded,
    Failed,
}

/// Tracks the progress of an asynchronous panel shutdown.
///
/// Created by [`crate::Board::begin_async_shutdown`] and polled each frame to
/// decide when the application can safely exit.
pub struct ShutdownProgress {
    started_at: Instant,
    panel_count: usize,
    terminal_joins_completed: Arc<AtomicUsize>,
    browser_count: usize,
    browser_shutdown_signals: Arc<Mutex<Vec<BrowserShutdownSignal>>>,
    browsers_completed: Arc<AtomicUsize>,
    forced_browser_shutdown_status: Arc<AtomicUsize>,
}

impl ShutdownProgress {
    pub(crate) fn new(
        panel_count: usize,
        terminal_joins_completed: Arc<AtomicUsize>,
        browser_shutdown_signals: Vec<BrowserShutdownSignal>,
    ) -> Self {
        let browser_count = browser_shutdown_signals.len();
        Self {
            started_at: Instant::now(),
            panel_count,
            terminal_joins_completed,
            browser_count,
            browser_shutdown_signals: Arc::new(Mutex::new(browser_shutdown_signals)),
            browsers_completed: Arc::new(AtomicUsize::new(0)),
            forced_browser_shutdown_status: Arc::new(AtomicUsize::new(FORCE_NOT_STARTED)),
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
        signals.retain(|signal| {
            if signal.is_complete() {
                self.browsers_completed.fetch_add(1, Ordering::Relaxed);
                false
            } else {
                true
            }
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

    /// Wait for normal browser teardown up to `timeout`, then force-terminate
    /// and reap every exact owned Chrome child within one final bounded
    /// window. Returns whether all browser ownership was released.
    #[must_use]
    pub fn wait_for_browser_shutdown(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.browser_shutdown_is_complete() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if self.browser_shutdown_is_complete() {
            return true;
        }
        match self.forced_browser_shutdown_status() {
            ForcedBrowserShutdownStatus::NotStarted => self.force_browser_shutdown(FORCED_BROWSER_SHUTDOWN_WAIT),
            ForcedBrowserShutdownStatus::Running => {
                let deadline = Instant::now() + FORCED_BROWSER_SHUTDOWN_WAIT;
                while self.forced_browser_shutdown_status() == ForcedBrowserShutdownStatus::Running
                    && Instant::now() < deadline
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                self.forced_browser_shutdown_status() == ForcedBrowserShutdownStatus::Succeeded
            }
            ForcedBrowserShutdownStatus::Succeeded => true,
            ForcedBrowserShutdownStatus::Failed => false,
        }
    }

    /// Start the one allowed forced browser cleanup attempt on a background
    /// thread and report its current state. The frame thread only starts and
    /// polls this work; it never waits on process termination or profile I/O.
    #[must_use]
    pub fn force_browser_shutdown_in_background(&self, timeout: Duration) -> ForcedBrowserShutdownStatus {
        if self
            .forced_browser_shutdown_status
            .compare_exchange(FORCE_NOT_STARTED, FORCE_RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let signals = Arc::clone(&self.browser_shutdown_signals);
            let completed = Arc::clone(&self.browsers_completed);
            let status = Arc::clone(&self.forced_browser_shutdown_status);
            let browser_count = self.browser_count;
            let spawn_result = std::thread::Builder::new()
                .name("browser-force-shutdown".to_string())
                .spawn(move || {
                    let succeeded = if let Ok(succeeded) =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            force_browser_shutdown_signals(&signals, &completed, browser_count, timeout)
                        })) {
                        succeeded
                    } else {
                        tracing::error!("forced browser cleanup thread panicked");
                        false
                    };
                    status.store(
                        if succeeded { FORCE_SUCCEEDED } else { FORCE_FAILED },
                        Ordering::Release,
                    );
                });
            if let Err(error) = spawn_result {
                tracing::error!("failed to start forced browser cleanup: {error}");
                self.forced_browser_shutdown_status
                    .store(FORCE_FAILED, Ordering::Release);
            }
        }
        self.forced_browser_shutdown_status()
    }

    #[must_use]
    pub fn forced_browser_shutdown_status(&self) -> ForcedBrowserShutdownStatus {
        match self.forced_browser_shutdown_status.load(Ordering::Acquire) {
            FORCE_NOT_STARTED => ForcedBrowserShutdownStatus::NotStarted,
            FORCE_RUNNING => ForcedBrowserShutdownStatus::Running,
            FORCE_SUCCEEDED => ForcedBrowserShutdownStatus::Succeeded,
            _ => ForcedBrowserShutdownStatus::Failed,
        }
    }

    /// Emergency process cleanup for frame-polled shutdown after its normal
    /// deadline. The timeout is shared across all pending browser processes.
    #[must_use]
    pub fn force_browser_shutdown(&self, timeout: Duration) -> bool {
        force_browser_shutdown_signals(
            &self.browser_shutdown_signals,
            &self.browsers_completed,
            self.browser_count,
            timeout,
        )
    }
}

fn force_browser_shutdown_signals(
    signals: &Mutex<Vec<BrowserShutdownSignal>>,
    browsers_completed: &AtomicUsize,
    browser_count: usize,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    let mut pending = {
        let mut signals = signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *signals)
    };
    pending.retain(|signal| {
        let complete = signal.is_complete() || signal.force_cleanup(deadline.saturating_duration_since(Instant::now()));
        if complete {
            browsers_completed.fetch_add(1, Ordering::Relaxed);
        }
        !complete
    });
    if !pending.is_empty() {
        signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .append(&mut pending);
    }
    browsers_completed.load(Ordering::Relaxed) >= browser_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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
        let progress = ShutdownProgress::new(
            2,
            Arc::new(AtomicUsize::new(0)),
            vec![BrowserShutdownSignal::for_test(completed_rx)],
        );

        assert!(!progress.browser_shutdown_is_complete());
        assert!(completed_tx.send(()).is_ok());
        assert!(progress.browser_shutdown_is_complete());
        assert!(!progress.is_complete());
    }

    #[test]
    fn browser_wait_forces_bounded_cleanup_after_timeout() {
        let (completed_tx, completed_rx) = mpsc::channel();
        // The driver is already gone: force cleanup must not wait on it.
        drop(completed_tx);
        let progress = ShutdownProgress::new(
            1,
            Arc::new(AtomicUsize::new(0)),
            vec![BrowserShutdownSignal::for_test(completed_rx)],
        );

        assert!(progress.wait_for_browser_shutdown(Duration::ZERO));
        assert!(progress.is_complete());
    }

    #[test]
    fn background_browser_force_is_one_shot_and_completes_without_blocking_the_caller() {
        let (completed_tx, completed_rx) = mpsc::channel();
        // The driver is already gone: force cleanup must not wait on it.
        drop(completed_tx);
        let progress = ShutdownProgress::new(
            1,
            Arc::new(AtomicUsize::new(0)),
            vec![BrowserShutdownSignal::for_test(completed_rx)],
        );
        let started = Instant::now();

        let initial = progress.force_browser_shutdown_in_background(Duration::from_secs(3));

        assert!(matches!(
            initial,
            ForcedBrowserShutdownStatus::Running | ForcedBrowserShutdownStatus::Succeeded
        ));
        assert!(started.elapsed() < Duration::from_millis(500));
        let deadline = Instant::now() + Duration::from_secs(1);
        while progress.forced_browser_shutdown_status() == ForcedBrowserShutdownStatus::Running
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(
            progress.forced_browser_shutdown_status(),
            ForcedBrowserShutdownStatus::Succeeded
        );
        assert_eq!(
            progress.force_browser_shutdown_in_background(Duration::from_secs(3)),
            ForcedBrowserShutdownStatus::Succeeded
        );
        assert!(progress.browser_shutdown_is_complete());
    }
}
