use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const PREPARE_FAILURE: &str = "failed to clear stale browser coordination; retry startup";

/// Live backend endpoint and page metadata exported to an optional host
/// coordination layer. Horizon uses this boundary for agent steering and
/// handoff; standalone users can omit coordination entirely.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoordinationState {
    pub backend: crate::BackendKind,
    pub browser_ws: String,
    pub target_id: String,
    pub url: String,
    pub title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffRequest {
    pub request_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoordinationSignals {
    pub owner: Option<String>,
    pub handoff: Option<HandoffRequest>,
    /// Validated, atomically claimed external actions. A host must not
    /// return the same request twice unless it intentionally wants a retry.
    pub actions: Vec<crate::AgentAction>,
}

/// Optional product-owned coordination boundary for live browser sessions.
/// Implementations must preserve concurrent external fields when updating
/// driver-owned state and must make `remove` bounded by the supplied timeout.
pub trait BrowserCoordination: Debug + Send + Sync + 'static {
    fn prepare(&self, panel_local_id: &str, timeout: Duration) -> bool;
    /// # Errors
    /// Returns an I/O error when initial coordination state cannot be stored.
    fn initialize(&self, panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when live coordination state cannot be updated.
    fn update(&self, panel_local_id: &str, state: &CoordinationState) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when the activity signal cannot be stored.
    fn set_user_active(&self, panel_local_id: &str, active: bool) -> std::io::Result<()>;
    /// # Errors
    /// Returns an I/O error when host-owned signals cannot be read.
    fn signals(&self, panel_local_id: &str) -> std::io::Result<CoordinationSignals>;
    /// # Errors
    /// Returns an I/O error when the exact handoff cannot be updated.
    fn acknowledge_handoff(&self, panel_local_id: &str, request_id: &str) -> std::io::Result<bool>;
    /// Append one privacy-aware action record to the host's audit sink.
    /// The default keeps audit optional for standalone embedders.
    ///
    /// # Errors
    /// Returns an I/O error when the configured audit sink cannot persist
    /// the record.
    fn record_action(&self, _panel_local_id: &str, _entry: &crate::BrowserAuditEntry) -> std::io::Result<()> {
        Ok(())
    }
    /// Publish the terminal result for an externally queued action. The
    /// default keeps result transport optional for embedders that only accept
    /// fire-and-forget controls.
    ///
    /// # Errors
    /// Returns an I/O error when the host cannot publish the result.
    fn complete_action(&self, _panel_local_id: &str, _result: &crate::AgentActionResult) -> std::io::Result<()> {
        Ok(())
    }
    /// Register one result for preservation while `remove` prunes other
    /// per-panel results. The driver calls this immediately before
    /// [`Self::complete_action`] publishes that result, so implementations
    /// must record the retention intent without requiring the result artifact
    /// to exist yet. Hosts that do not persist results can ignore this hint.
    fn retain_action_result_on_remove(&self, _panel_local_id: &str, _action_id: &str) {}
    /// Apply the host's retention policy before the engine creates an
    /// explicit network export. The default leaves storage policy with
    /// standalone embedders that do not need persistent capture directories.
    ///
    /// # Errors
    /// Returns an I/O error when the host cannot make enough bounded storage
    /// available for the requested capture.
    fn prepare_network_capture(
        &self,
        _panel_local_id: &str,
        _directory: &Path,
        _requested_max_file_bytes: u64,
    ) -> std::io::Result<()> {
        Ok(())
    }
    fn remove(&self, panel_local_id: &str, timeout: Duration) -> bool;
}

/// Own the host coordination entry for exactly one driver lifetime.
pub(crate) struct CoordinationLifetime {
    coordination: Option<Arc<dyn BrowserCoordination>>,
    panel_local_id: String,
}

impl CoordinationLifetime {
    pub(crate) fn start(config: &crate::BrowserSessionConfig) -> Option<Self> {
        if let Some(coordination) = &config.coordination
            && !coordination.prepare(&config.panel_local_id, Duration::from_secs(2))
        {
            tracing::warn!(target: "browser", "failed to remove stale browser coordination before startup");
            return None;
        }
        Some(Self {
            coordination: config.coordination.clone(),
            panel_local_id: config.panel_local_id.clone(),
        })
    }
}

impl Drop for CoordinationLifetime {
    fn drop(&mut self) {
        if let Some(coordination) = &self.coordination {
            let _ = coordination.remove(&self.panel_local_id, Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[derive(Debug)]
    struct RefusingCoordination;

    impl BrowserCoordination for RefusingCoordination {
        fn prepare(&self, _panel_local_id: &str, _timeout: Duration) -> bool {
            false
        }

        fn initialize(&self, _panel_local_id: &str, _state: &CoordinationState) -> std::io::Result<()> {
            Ok(())
        }

        fn update(&self, _panel_local_id: &str, _state: &CoordinationState) -> std::io::Result<()> {
            Ok(())
        }

        fn set_user_active(&self, _panel_local_id: &str, _active: bool) -> std::io::Result<()> {
            Ok(())
        }

        fn signals(&self, _panel_local_id: &str) -> std::io::Result<CoordinationSignals> {
            Ok(CoordinationSignals::default())
        }

        fn acknowledge_handoff(&self, _panel_local_id: &str, _request_id: &str) -> std::io::Result<bool> {
            Ok(false)
        }

        fn remove(&self, _panel_local_id: &str, _timeout: Duration) -> bool {
            true
        }
    }

    #[test]
    fn failed_stale_state_cleanup_aborts_coordination_startup() {
        let config = crate::BrowserSessionConfig {
            browser: crate::BrowserConfig::default(),
            panel_local_id: "panel".into(),
            initial_url: None,
            width: 1,
            height: 1,
            frame_slot: Arc::new(crate::FrameSlot::new()),
            coordination: Some(Arc::new(RefusingCoordination)),
            capture_directory: None,
        };

        assert!(CoordinationLifetime::start(&config).is_none());
    }
}
