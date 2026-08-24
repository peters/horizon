use std::time::Duration;

use horizon_core::ForcedBrowserShutdownStatus;

use super::HorizonApp;
use crate::loading_spinner;

const MAX_SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
const FORCED_BROWSER_SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserShutdownOutcome {
    Pending,
    Complete,
    ForcedCleanupFailed,
}

const fn shutdown_ready_to_exit(complete: bool, timed_out: bool, browser: BrowserShutdownOutcome) -> bool {
    match browser {
        BrowserShutdownOutcome::Pending => false,
        BrowserShutdownOutcome::Complete => complete || timed_out,
        BrowserShutdownOutcome::ForcedCleanupFailed => timed_out,
    }
}

impl HorizonApp {
    /// Starts asynchronous panel shutdown. State is saved immediately, and
    /// background threads join terminal event loops and browser drivers.
    #[profiling::function]
    pub(in crate::app) fn begin_shutdown(&mut self) {
        if self.shutdown_progress.is_some() {
            return;
        }

        let _ = self.drain_panel_output();
        let _ = self.auto_save_runtime_state();
        self.git_watchers.clear();
        let mut progress = self
            .pending_session_switch
            .take()
            .map(|pending| pending.shutdown_progress)
            .unwrap_or_else(|| self.board.begin_async_shutdown());
        progress.restart_timeout_window();
        self.shutdown_progress = Some(progress);
    }

    #[profiling::function]
    pub(in crate::app) fn poll_shutdown_progress(&mut self) {
        let Some(progress) = &self.shutdown_progress else {
            return;
        };
        let complete = progress.is_complete();
        let timed_out = progress.started_at().elapsed() > MAX_SHUTDOWN_WAIT;
        let mut browser_outcome = if progress.browser_shutdown_is_complete() {
            BrowserShutdownOutcome::Complete
        } else {
            BrowserShutdownOutcome::Pending
        };
        if timed_out && browser_outcome == BrowserShutdownOutcome::Pending {
            browser_outcome = match progress.force_browser_shutdown_in_background(FORCED_BROWSER_SHUTDOWN_WAIT) {
                ForcedBrowserShutdownStatus::NotStarted | ForcedBrowserShutdownStatus::Running => return,
                ForcedBrowserShutdownStatus::Succeeded => BrowserShutdownOutcome::Complete,
                ForcedBrowserShutdownStatus::Failed => BrowserShutdownOutcome::ForcedCleanupFailed,
            };
        }
        if !shutdown_ready_to_exit(complete, timed_out, browser_outcome) {
            return;
        }

        // Browser sessions publish their committed URL before resolving the
        // teardown signal. Persist that final state only after every driver
        // is known to be finished.
        let _ = self.drain_panel_output();
        let _ = self.auto_save_runtime_state();
        self.exit_cleanup_complete = true;
        self.release_active_session_lease();
        if browser_outcome == BrowserShutdownOutcome::ForcedCleanupFailed {
            tracing::error!("forced browser cleanup failed; exiting after the bounded shutdown deadline");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    #[profiling::function]
    pub(in crate::app) fn render_shutdown_overlay(&self, ui: &mut egui::Ui) {
        let Some(progress) = &self.shutdown_progress else {
            return;
        };
        let completed = progress.panels_completed();
        let total = progress.panel_count();

        egui::CentralPanel::default().show(ui, |ui| {
            if total > 0 {
                loading_spinner::show_with_detail(
                    ui,
                    egui::Id::new("shutdown_spinner"),
                    "Closing Horizon\u{2026}",
                    &format!("{completed} / {total} panels shut down"),
                );
            } else {
                loading_spinner::show(ui, egui::Id::new("shutdown_spinner"), Some("Closing Horizon\u{2026}"));
            }
        });
    }

    /// Synchronous fallback for the `on_exit` eframe callback.
    #[profiling::function]
    pub(in crate::app) fn run_exit_cleanup(&mut self) {
        if self.exit_cleanup_complete {
            return;
        }

        self.exit_cleanup_complete = true;
        let _ = self.drain_panel_output();
        let _ = self.auto_save_runtime_state();
        if let Some(progress) = self
            .pending_session_switch
            .take()
            .map(|pending| pending.shutdown_progress)
        {
            if !progress.wait_for_browser_shutdown(MAX_SHUTDOWN_WAIT) {
                tracing::warn!("failed to terminate every browser during session-switch exit cleanup");
            }
            if !progress.wait_for_completion(MAX_SHUTDOWN_WAIT) {
                tracing::warn!(
                    completed = progress.panels_completed(),
                    total = progress.panel_count(),
                    "timed out waiting for session-switch terminal shutdown during exit"
                );
            }
        }
        self.board.shutdown_terminal_panels();
        if let Some(progress) = &self.shutdown_progress {
            if !progress.wait_for_browser_shutdown(MAX_SHUTDOWN_WAIT) {
                tracing::warn!("failed to terminate every browser during asynchronous exit cleanup");
            }
            if !progress.wait_for_completion(MAX_SHUTDOWN_WAIT) {
                tracing::warn!(
                    completed = progress.panels_completed(),
                    total = progress.panel_count(),
                    "timed out waiting for asynchronous terminal shutdown during exit"
                );
            }
        }
        let _ = self.drain_panel_output();
        let _ = self.auto_save_runtime_state();
        self.git_watchers.clear();
        self.release_active_session_lease();
    }
}

#[cfg(test)]
mod tests {
    use super::{BrowserShutdownOutcome, shutdown_ready_to_exit};

    #[test]
    fn timeout_never_bypasses_browser_teardown() {
        assert!(!shutdown_ready_to_exit(false, true, BrowserShutdownOutcome::Pending));
        assert!(!shutdown_ready_to_exit(true, false, BrowserShutdownOutcome::Pending));
        assert!(shutdown_ready_to_exit(false, true, BrowserShutdownOutcome::Complete));
        assert!(shutdown_ready_to_exit(true, false, BrowserShutdownOutcome::Complete));
    }

    #[test]
    fn failed_forced_cleanup_has_a_bounded_terminal_outcome() {
        assert!(!shutdown_ready_to_exit(
            false,
            false,
            BrowserShutdownOutcome::ForcedCleanupFailed
        ));
        assert!(shutdown_ready_to_exit(
            false,
            true,
            BrowserShutdownOutcome::ForcedCleanupFailed
        ));
    }
}
