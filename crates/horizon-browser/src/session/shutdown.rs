//! Bounded browser-process and profile teardown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::BrowserCoordination;
use crate::process::ChromeProcessControl;

/// Completion signal paired with an exact browser child handle. Normal paths
/// only poll the receiver; a hard application-exit deadline can explicitly
/// terminate and reap the owned process before removing its manifest.
pub struct BrowserShutdownSignal {
    completion_rx: mpsc::Receiver<()>,
    driver_complete: AtomicBool,
    process_complete: AtomicBool,
    process_control: ChromeProcessControl,
    panel_local_id: Option<String>,
    coordination: Option<Arc<dyn BrowserCoordination>>,
    profile_cleanup: Mutex<ProfileCleanupState>,
}

enum ProfileCleanupState {
    NotRequired,
    Pending(std::path::PathBuf),
    Running {
        profile_dir: std::path::PathBuf,
        completion_rx: mpsc::Receiver<std::io::Result<()>>,
    },
    RetryPending {
        profile_dir: std::path::PathBuf,
        retry_at: Instant,
    },
}

const PROFILE_CLEANUP_RETRY_DELAY: Duration = Duration::from_secs(1);

impl BrowserShutdownSignal {
    pub(super) fn running(
        completion_rx: mpsc::Receiver<()>,
        process_control: ChromeProcessControl,
        panel_local_id: String,
        coordination: Option<Arc<dyn BrowserCoordination>>,
    ) -> Self {
        Self {
            completion_rx,
            driver_complete: AtomicBool::new(false),
            process_complete: AtomicBool::new(false),
            process_control,
            panel_local_id: Some(panel_local_id),
            coordination,
            profile_cleanup: Mutex::new(ProfileCleanupState::NotRequired),
        }
    }

    #[must_use]
    pub fn with_profile_cleanup(self, profile_dir: std::path::PathBuf) -> Self {
        *self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ProfileCleanupState::Pending(profile_dir);
        self
    }

    #[must_use]
    pub fn completed_with_profile_cleanup(profile_dir: std::path::PathBuf) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        drop(completion_tx);
        let process_control = ChromeProcessControl::default();
        process_control.mark_registration_settled();
        Self {
            completion_rx,
            driver_complete: AtomicBool::new(true),
            process_complete: AtomicBool::new(true),
            process_control,
            panel_local_id: None,
            coordination: None,
            profile_cleanup: Mutex::new(ProfileCleanupState::Pending(profile_dir)),
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.process_is_complete() && self.profile_cleanup_is_complete()
    }

    #[must_use]
    pub fn wait(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        if !self.wait_driver_completion(timeout) {
            return false;
        }
        while !self.process_control.is_reaped() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !self.process_control.is_reaped() {
            return false;
        }
        self.process_complete.store(true, Ordering::Release);
        self.wait_for_profile_cleanup(deadline.saturating_duration_since(Instant::now()))
    }

    /// Emergency cleanup after the normal driver deadline. Returns only after
    /// the browser has been reaped (or no child was spawned) and any permanent
    /// panel-close profile removal has finished.
    #[must_use]
    pub fn force_cleanup(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        if !self
            .process_control
            .terminate(deadline.saturating_duration_since(Instant::now()))
        {
            return false;
        }
        // The driver may still be running between the browser's death and its
        // own exit; wait before removing files it owns or it could recreate
        // the manifest after we delete it.
        if !self.wait_driver_completion(deadline.saturating_duration_since(Instant::now())) {
            return false;
        }
        if let (Some(panel_local_id), Some(coordination)) = (&self.panel_local_id, &self.coordination)
            && !coordination.remove(panel_local_id, deadline.saturating_duration_since(Instant::now()))
        {
            return false;
        }
        self.process_complete.store(true, Ordering::Release);
        self.retry_failed_profile_cleanup_now();
        self.wait_for_profile_cleanup(deadline.saturating_duration_since(Instant::now()))
    }

    fn wait_driver_completion(&self, timeout: Duration) -> bool {
        if self.driver_complete.load(Ordering::Acquire) {
            return true;
        }
        let complete = matches!(
            self.completion_rx.recv_timeout(timeout),
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)
        );
        if complete {
            self.driver_complete.store(true, Ordering::Release);
        }
        complete
    }

    fn process_is_complete(&self) -> bool {
        if self.process_complete.load(Ordering::Acquire) {
            return true;
        }
        let driver_complete = self.driver_complete.load(Ordering::Acquire)
            || matches!(
                self.completion_rx.try_recv(),
                Ok(()) | Err(mpsc::TryRecvError::Disconnected)
            );
        if driver_complete {
            self.driver_complete.store(true, Ordering::Release);
        }
        let complete = driver_complete && self.process_control.is_reaped();
        if complete {
            self.process_complete.store(true, Ordering::Release);
        }
        complete
    }

    fn profile_cleanup_is_complete(&self) -> bool {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        poll_profile_cleanup(&mut cleanup, None)
    }

    fn wait_for_profile_cleanup(&self, timeout: Duration) -> bool {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        poll_profile_cleanup(&mut cleanup, Some(timeout))
    }

    fn retry_failed_profile_cleanup_now(&self) {
        let mut cleanup = self
            .profile_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = std::mem::replace(&mut *cleanup, ProfileCleanupState::NotRequired);
        *cleanup = match previous {
            ProfileCleanupState::RetryPending { profile_dir, .. } => ProfileCleanupState::Pending(profile_dir),
            other => other,
        };
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn for_test(completion_rx: mpsc::Receiver<()>) -> Self {
        let process_control = ChromeProcessControl::default();
        process_control.mark_registration_settled();
        Self {
            completion_rx,
            driver_complete: AtomicBool::new(false),
            process_complete: AtomicBool::new(false),
            process_control,
            panel_local_id: None,
            coordination: None,
            profile_cleanup: Mutex::new(ProfileCleanupState::NotRequired),
        }
    }
}

fn start_profile_cleanup(cleanup: &mut ProfileCleanupState) {
    let previous = std::mem::replace(cleanup, ProfileCleanupState::NotRequired);
    let profile_dir = match previous {
        ProfileCleanupState::Pending(profile_dir) => profile_dir,
        ProfileCleanupState::RetryPending { profile_dir, retry_at } if Instant::now() >= retry_at => profile_dir,
        other => {
            *cleanup = other;
            return;
        }
    };
    let completion_rx = crate::profile::schedule_removal(profile_dir.clone());
    *cleanup = ProfileCleanupState::Running {
        profile_dir,
        completion_rx,
    };
}

fn poll_profile_cleanup(cleanup: &mut ProfileCleanupState, timeout: Option<Duration>) -> bool {
    start_profile_cleanup(cleanup);
    let outcome = match cleanup {
        ProfileCleanupState::NotRequired => return true,
        ProfileCleanupState::Pending(_) | ProfileCleanupState::RetryPending { .. } => return false,
        ProfileCleanupState::Running { completion_rx, .. } => match timeout {
            Some(timeout) => match completion_rx.recv_timeout(timeout) {
                Ok(result) => Some(result),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(Err(std::io::Error::other(
                    "browser profile cleanup worker disconnected",
                ))),
            },
            None => match completion_rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(std::io::Error::other(
                    "browser profile cleanup worker disconnected",
                ))),
            },
        },
    };
    let Some(outcome) = outcome else {
        return false;
    };
    match outcome {
        Ok(()) => {
            *cleanup = ProfileCleanupState::NotRequired;
            true
        }
        Err(error) => {
            let ProfileCleanupState::Running { profile_dir, .. } =
                std::mem::replace(cleanup, ProfileCleanupState::NotRequired)
            else {
                return false;
            };
            tracing::warn!(path = %profile_dir.display(), "failed to remove browser profile: {error}");
            *cleanup = ProfileCleanupState::RetryPending {
                profile_dir,
                retry_at: Instant::now() + PROFILE_CLEANUP_RETRY_DELAY,
            };
            false
        }
    }
}
