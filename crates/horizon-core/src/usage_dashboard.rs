use std::sync::mpsc;

use crate::usage_stats::{UsageSnapshot, spawn_usage_poll};

/// Panel content for the Usage dashboard.
///
/// Holds a background-polling receiver that delivers periodic snapshots
/// of Claude Code and Codex CLI usage statistics.
pub struct UsageDashboard {
    rx: Option<mpsc::Receiver<UsageSnapshot>>,
    pub snapshot: Option<UsageSnapshot>,
}

impl UsageDashboard {
    /// Create a new dashboard and start background polling.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rx: Some(spawn_usage_poll()),
            snapshot: None,
        }
    }

    /// Check for a new snapshot from the background poll thread.
    /// Returns `true` if a new snapshot was received.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                false
            }
        }
    }
}

impl Default for UsageDashboard {
    fn default() -> Self {
        Self::new()
    }
}
