//! The remote-session side of a browser panel: the request it (re)launches
//! with, the configured target it runs at, and whether the provider still
//! holds the session. Local panels never enter this module.

use horizon_browser::{BackendKind, BrowserConfig, RemoteSessionRequest, normalize_navigation_target};

use super::{BrowserDrainOutput, BrowserPanelState, BrowserStatus, retain_effective_profile_root};

/// What a panel knows about the remote session it runs (or ran) at.
pub(super) struct RemoteLifecycle {
    /// The request every (re)launch uses. `None` for a panel restored from
    /// a previous run, which cannot allocate again on its own.
    request: Option<RemoteSessionRequest>,
    /// Configured remote target name.
    target: String,
    /// The driver established that the provider no longer holds this
    /// session (released, already gone, or never allocated).
    release_established: bool,
}

impl RemoteLifecycle {
    fn live(request: RemoteSessionRequest) -> Self {
        Self {
            target: request.label.clone(),
            request: Some(request),
            release_established: false,
        }
    }

    const fn restored(target: String) -> Self {
        Self {
            request: None,
            target,
            release_established: false,
        }
    }
}

impl BrowserPanelState {
    /// Create the state for a remote session and start allocating. The
    /// request is kept for Retry, so a retried panel reuses the authorization
    /// resolved at creation instead of consulting a store again. The panel's
    /// backend is the request's browser family, not the local default, so
    /// coordination and the MCP projection describe the device's browser.
    ///
    /// # Errors
    /// Returns an error if a relative profile root cannot be resolved from
    /// the process launch directory.
    pub fn start_remote(
        panel_local_id: impl Into<String>,
        config: &BrowserConfig,
        initial_url: Option<String>,
        request: RemoteSessionRequest,
    ) -> crate::error::Result<Self> {
        let panel_local_id = panel_local_id.into();
        let mut config = config.resolved_for_launch()?;
        let home = crate::horizon_home::HorizonHome::resolve();
        let profile_root_resolved = retain_effective_profile_root(&mut config, &home.root().join("browser-profiles"));
        config.backend = request.browser;
        let initial_url = initial_url.map(|url| normalize_navigation_target(&url));
        let mut state = Self::inert();
        state.panel_local_id = panel_local_id;
        state.status = BrowserStatus::Starting;
        state.loading = true;
        state.requested_url.clone_from(&initial_url);
        state.persisted_config_changed = profile_root_resolved;
        state.config = config;
        state.remote = Some(RemoteLifecycle::live(request));
        state.launch_session(initial_url);
        Ok(state)
    }

    /// A remote panel restored from a previous run. Its device session ended
    /// with that run and the credentials were resolved then, so it comes back
    /// stopped with a note; a new session needs a new create.
    #[must_use]
    pub fn restored_remote(
        panel_local_id: impl Into<String>,
        config: &BrowserConfig,
        remote_target: String,
        last_url: Option<String>,
    ) -> Self {
        let mut state = Self::inert();
        state.panel_local_id = panel_local_id.into();
        state.config = config.clone();
        state.requested_url = last_url;
        state.remote_status = Some(format!(
            "remote session for {remote_target} ended with the previous Horizon run; create the panel again to allocate a new one"
        ));
        state.remote = Some(RemoteLifecycle::restored(remote_target));
        state
    }

    /// Configured remote target name, when this panel runs (or ran) remotely.
    #[must_use]
    pub fn remote_target(&self) -> Option<&str> {
        self.remote.as_ref().map(|remote| remote.target.as_str())
    }

    /// Whether this panel drives a remote session rather than a local browser.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Configured provider of the remote session this panel runs at.
    #[must_use]
    pub fn remote_provider(&self) -> Option<&str> {
        self.remote
            .as_ref()
            .and_then(|remote| remote.request.as_ref())
            .map(|request| request.provider.as_str())
    }

    /// Whether this panel may still hold an allocation at its provider: a
    /// remote session whose release (or allocation failure) the driver has
    /// not positively established. A restored panel without a request holds
    /// nothing; its session ended with the previous run.
    #[must_use]
    pub fn holds_remote_allocation(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.request.is_some() && !remote.release_established)
    }

    /// Driver-less remote panel for host tests: counts as holding an
    /// allocation at `provider` until marked released.
    #[doc(hidden)]
    #[must_use]
    pub fn inert_remote(target: &str, provider: &str) -> Self {
        let mut state = Self::inert();
        state.status = BrowserStatus::Ready;
        state.remote = Some(RemoteLifecycle::live(RemoteSessionRequest {
            endpoint: "https://grid.example.net/wd/hub".to_string(),
            authorization: None,
            capabilities: serde_json::json!({}),
            allocation_timeout: std::time::Duration::from_secs(1),
            max_session: std::time::Duration::from_secs(1),
            idle_release: std::time::Duration::from_secs(1),
            label: target.to_string(),
            provider: provider.to_string(),
            browser: BackendKind::ChromiumCdp,
        }));
        state
    }

    /// Pretend the driver reported the release established, for host tests.
    #[doc(hidden)]
    pub fn mark_remote_release_established_for_tests(&mut self) {
        if let Some(remote) = self.remote.as_mut() {
            remote.release_established = true;
        }
    }

    /// Whether Retry can start a session again. A remote panel restored from
    /// a previous run has no request to retry with: it needs a new create.
    #[must_use]
    pub fn can_retry(&self) -> bool {
        !self.is_restored_remote()
    }

    /// A remote panel restored from a previous run: known target, no request.
    pub(super) fn is_restored_remote(&self) -> bool {
        self.remote.as_ref().is_some_and(|remote| remote.request.is_none())
    }

    /// The request the next driver start uses, when this panel is remote.
    pub(super) fn remote_request(&self) -> Option<RemoteSessionRequest> {
        self.remote.as_ref().and_then(|remote| remote.request.clone())
    }

    pub(super) fn apply_remote_session_event(
        &mut self,
        event: horizon_browser::RemoteSessionEvent,
        output: &mut BrowserDrainOutput,
    ) {
        use horizon_browser::{RemoteExpiry, RemoteReleaseOutcome, RemoteSessionEvent};
        // Only a positive answer frees the provider's slot: an allocation the
        // provider refused, or a delete it accepted or no longer knows.
        let released = matches!(
            &event,
            RemoteSessionEvent::AllocationFailed { .. }
                | RemoteSessionEvent::Released {
                    outcome: RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone,
                    ..
                }
        );
        let note = match event {
            RemoteSessionEvent::Allocating { label } => format!("allocating remote device for {label}"),
            RemoteSessionEvent::Allocated { label, session_digest } => {
                format!("remote session {session_digest} allocated for {label}")
            }
            RemoteSessionEvent::AllocationUnknown { label, reason } => {
                format!("remote allocation for {label} is unknown ({reason}); check the provider before retrying")
            }
            RemoteSessionEvent::AllocationFailed { label, reason } => {
                format!("remote allocation for {label} failed: {reason}")
            }
            RemoteSessionEvent::Expired { label, reason } => match reason {
                RemoteExpiry::HardDeadline => format!("remote session for {label} reached its maximum lifetime"),
                RemoteExpiry::Idle => format!("remote session for {label} was released after idling"),
            },
            RemoteSessionEvent::Released { label, outcome } => match outcome {
                RemoteReleaseOutcome::Released => format!("remote session for {label} released"),
                RemoteReleaseOutcome::AlreadyGone => format!("remote session for {label} was already gone"),
                RemoteReleaseOutcome::ReleaseUnknown { attempts, reason } => format!(
                    "remote session for {label} may still be held: {attempts} release attempts failed ({reason})"
                ),
                RemoteReleaseOutcome::Failed { error, message } => {
                    format!("remote session for {label} could not be released ({error}: {message})")
                }
            },
        };
        tracing::info!(target: "browser", "{note}");
        self.remote_status = Some(note);
        if released && let Some(remote) = self.remote.as_mut() {
            remote.release_established = true;
        }
        output.had_output = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BackendCapabilities, BackendKind, BrowserConfig, BrowserPanelState, BrowserStatus};

    #[test]
    fn restored_remote_panels_stay_stopped_and_keep_their_target() {
        let mut state = BrowserPanelState::restored_remote(
            "remote-restore",
            &BrowserConfig::default(),
            "ios_phone".to_string(),
            Some("https://example.test/".to_string()),
        );
        assert!(state.is_remote());
        assert_eq!(
            state.backend_capabilities(),
            BackendCapabilities::remote_session(),
            "a remote panel never advertises a local backend's capabilities"
        );
        assert!(!state.can_retry(), "a request-less remote panel offers no Retry");
        assert_eq!(state.remote_target(), Some("ios_phone"));
        assert!(matches!(state.status, BrowserStatus::Stopped { code: None }));
        assert!(
            state
                .remote_status
                .as_deref()
                .is_some_and(|note| note.contains("ios_phone") && note.contains("create the panel again")),
            "{:?}",
            state.remote_status
        );
        assert_eq!(state.display_url(), "https://example.test/");

        state.relaunch();
        assert!(state.session.is_none(), "no local browser stands in for the device");
        assert!(matches!(state.status, BrowserStatus::Stopped { code: None }));
        assert!(!state.loading);

        let backend = state.backend();
        state.switch_backend(BackendKind::FirefoxBidi);
        assert_eq!(
            state.backend(),
            backend,
            "a remote panel's browser is fixed by its target"
        );
        assert!(state.session.is_none());
    }
}
