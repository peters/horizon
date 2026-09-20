//! The remote-session side of a browser panel: the request it (re)launches
//! with, the configured target it runs at, and whether the provider still
//! holds the session. Local panels never enter this module.

use horizon_browser::{BackendKind, BrowserConfig, RemoteSessionRequest, normalize_navigation_target};

use super::{
    BrowserDrainOutput, BrowserPanelState, BrowserStatus, RemoteIdentityDisplay, retain_effective_profile_root,
};

/// What a panel knows about the remote session it runs (or ran) at.
pub(super) struct RemoteLifecycle {
    /// The request the first launch consumes. A remote panel allocates
    /// exactly once: Retry would bypass the host's provider limit, so a
    /// stopped panel needs a new create, which goes through that check.
    request: Option<RemoteSessionRequest>,
    recovery: Option<horizon_browser::RemoteAllocation>,
    /// Configured remote target name.
    target: String,
    /// Configured provider the session was (or is being) allocated at.
    /// `None` for a panel restored from a previous run, whose session ended
    /// with that run.
    provider: Option<String>,
    /// The provider identity the allocation counts against across Horizon
    /// instances; kept with the session, not looked up from configuration.
    quota_key: Option<String>,
    /// The driver established that the provider no longer holds this
    /// session (released, already gone, or never allocated).
    release_established: bool,
    /// The allocated device as the provider's evidence describes it, once
    /// the driver verified it against the target.
    device: Option<String>,
    identity_display: RemoteIdentityDisplay,
    /// Why the remote lifecycle ended before the panel became ready, as a
    /// typed code and a value-free message for the create result.
    failure: Option<RemoteFailure>,
}

/// A terminal remote lifecycle failure the host reports to the agent: a
/// typed code and fixed public text. Provider-reported detail stays in the
/// panel note and the local log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteFailure {
    pub code: &'static str,
    pub message: &'static str,
}

impl RemoteFailure {
    const DEVICE_REJECTED: Self = Self {
        code: "remote_device_rejected",
        message: "the allocated remote device did not meet the target's device requirement and Horizon attempted to release the session; the panel shows the failed requirement and whether the provider confirmed the release, so check it before creating again",
    };
    const ALLOCATION_FAILED: Self = Self {
        code: "remote_allocation_failed",
        message: "a remote session for the target could not be allocated or safely started and nothing is held at the provider; the panel shows the reason",
    };
    const AUTHENTICATION_FAILED: Self = Self {
        code: "remote_authentication_failed",
        message: "the provider rejected the configured credential for the target's provider and nothing is held; check the credential in Settings > Remote browsers",
    };
    const NOT_ENTITLED: Self = Self {
        code: "remote_not_entitled",
        message: "the provider account is not entitled to automate on this service, so no device was allocated; the plan or product access must change before creating again",
    };
    const DEVICE_UNAVAILABLE: Self = Self {
        code: "remote_device_unavailable",
        message: "the provider had no device matching the target, or none free, and nothing is held; check the target's device fields or the provider's capacity before creating again",
    };

    /// The typed failure for a provider refusal, by what it was about.
    const fn for_refusal(refusal: horizon_browser::AllocationRefusal) -> Self {
        match refusal {
            horizon_browser::AllocationRefusal::Authentication => Self::AUTHENTICATION_FAILED,
            horizon_browser::AllocationRefusal::Entitlement => Self::NOT_ENTITLED,
            horizon_browser::AllocationRefusal::DeviceUnavailable => Self::DEVICE_UNAVAILABLE,
            horizon_browser::AllocationRefusal::Other => Self::ALLOCATION_FAILED,
        }
    }
    const ALLOCATION_UNKNOWN: Self = Self {
        code: "remote_allocation_unknown",
        message: "the provider gave no trustworthy answer about the remote allocation or its cleanup, so a device may still be held; check the provider before creating again",
    };
}

impl RemoteLifecycle {
    fn live(request: RemoteSessionRequest) -> Self {
        Self {
            identity_display: RemoteIdentityDisplay::requested(&request),
            target: request.label.clone(),
            provider: Some(request.provider.clone()),
            quota_key: Some(request.quota_key.clone()),
            recovery: Some(request.recovery.clone()),
            request: Some(request),
            release_established: false,
            device: None,
            failure: None,
        }
    }

    fn restored(target: String) -> Self {
        Self {
            request: None,
            identity_display: RemoteIdentityDisplay::ended(),
            recovery: None,
            target,
            provider: None,
            quota_key: None,
            release_established: false,
            device: None,
            failure: None,
        }
    }
}

impl BrowserPanelState {
    /// Create the state for a remote session and start allocating. The
    /// request is consumed by this one launch: the host checked the
    /// provider's limit for it, and a Retry would not. The panel's backend
    /// is the request's browser family, not the local default, so
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

    /// The exact allocation belonging to this panel instance; restored panels have none.
    #[must_use]
    pub fn remote_allocation(&self) -> Option<&horizon_browser::RemoteAllocation> {
        self.remote.as_ref().and_then(|remote| remote.recovery.as_ref())
    }

    /// Configured remote target name, when this panel runs (or ran) remotely.
    #[must_use]
    pub fn remote_target(&self) -> Option<&str> {
        #[cfg(feature = "cloud-workspaces")]
        if let Some(cloud) = &self.cloud {
            return cloud.target.as_deref();
        }
        self.remote.as_ref().map(|remote| remote.target.as_str())
    }

    /// Whether this panel drives a remote session rather than a local browser.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        if self.cloud.is_some() {
            return true;
        }
        self.remote.is_some()
    }

    /// The allocated device as the provider's evidence describes it, once
    /// verified; `None` before verification or for a local panel.
    #[must_use]
    pub fn remote_device(&self) -> Option<&str> {
        #[cfg(feature = "cloud-workspaces")]
        if let Some(cloud) = &self.cloud {
            return cloud.device.as_deref();
        }
        self.remote.as_ref().and_then(|remote| remote.device.as_deref())
    }

    /// Cached provider and browser identity for the panel chrome.
    #[must_use]
    pub fn remote_identity_display(&self) -> Option<&RemoteIdentityDisplay> {
        self.remote.as_ref().map(|remote| &remote.identity_display)
    }

    pub(super) fn clear_remote_identity(&mut self) {
        if let Some(remote) = self.remote.as_mut() {
            remote.device = None;
            remote
                .identity_display
                .clear(remote.provider.is_none() || remote.release_established);
        }
    }

    /// Why the remote lifecycle ended before the panel became ready, when
    /// it did: the typed code and message the create result reports.
    #[must_use]
    pub fn remote_failure(&self) -> Option<&RemoteFailure> {
        self.remote.as_ref().and_then(|remote| remote.failure.as_ref())
    }

    /// The provider identity this panel's allocation counts against across
    /// Horizon instances, when it holds one.
    #[must_use]
    pub fn remote_quota_key(&self) -> Option<&str> {
        self.remote.as_ref().and_then(|remote| remote.quota_key.as_deref())
    }

    /// Configured provider of the remote session this panel runs at.
    #[must_use]
    pub fn remote_provider(&self) -> Option<&str> {
        self.remote.as_ref().and_then(|remote| remote.provider.as_deref())
    }

    /// Whether this panel may still hold an allocation at its provider: a
    /// remote session whose release (or allocation failure) the driver has
    /// not positively established. A restored panel without a request holds
    /// nothing; its session ended with the previous run.
    #[must_use]
    pub fn holds_remote_allocation(&self) -> bool {
        self.remote.as_ref().is_some_and(|remote| {
            remote.provider.is_some()
                && !remote.release_established
                && !remote
                    .recovery
                    .as_ref()
                    .is_some_and(horizon_browser::RemoteAllocation::is_released)
        })
    }

    /// Driver-less remote panel for host tests: counts as holding an
    /// allocation at `provider` until marked released.
    #[doc(hidden)]
    #[must_use]
    pub fn inert_remote(target: &str, provider: &str) -> Self {
        let mut state = Self::inert();
        state.status = BrowserStatus::Ready;
        state.remote = Some(RemoteLifecycle::live(RemoteSessionRequest {
            recovery: horizon_browser::RemoteAllocation::default(),
            endpoint: "https://grid.example.net/wd/hub".to_string(),
            authorization: None,
            capabilities: serde_json::json!({}),
            allocation_timeout: std::time::Duration::from_secs(1),
            max_session: std::time::Duration::from_secs(1),
            idle_release: std::time::Duration::from_secs(1),
            label: target.to_string(),
            provider: provider.to_string(),
            quota_key: provider.to_string(),
            browser: BackendKind::ChromiumCdp,
            device: horizon_browser::remote::DeviceRequirement::default(),
            evidence: horizon_browser::DeviceEvidenceSource::Capabilities,
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

    /// Whether Retry can start a session again. A remote panel never
    /// retries: only a new create passes the host's provider limit and
    /// release checks, and a retry after an unknown allocation could
    /// duplicate a session the provider still holds.
    #[must_use]
    pub fn can_retry(&self) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        if let Some(cloud) = &self.cloud {
            return !cloud.process_lost;
        }
        !self.is_remote()
    }

    /// The request this launch consumes, when the panel is remote and has
    /// not launched yet. `Err` for a remote panel with nothing to launch
    /// with (restored, or already launched once).
    pub(super) fn take_remote_request(&mut self) -> Result<Option<RemoteSessionRequest>, ()> {
        match self.remote.as_mut() {
            None => Ok(None),
            Some(remote) => remote.request.take().map(Some).ok_or(()),
        }
    }

    /// Refuse to start anything for a remote panel that has no request:
    /// no local browser may stand in for the device and nothing may be
    /// allocated outside the host's create path.
    pub(super) fn refuse_remote_relaunch(&mut self) {
        self.pending_relaunch = None;
        self.loading = false;
        self.status = BrowserStatus::Stopped { code: None };
        if self.remote_status.is_none() {
            let target = self.remote_target().unwrap_or_default();
            self.remote_status = Some(format!(
                "remote session for {target} cannot be retried here; create the panel again to allocate a new one"
            ));
        }
    }

    /// Feed one lifecycle event to the panel, for host tests of the typed
    /// outcomes it derives.
    #[doc(hidden)]
    pub fn apply_remote_session_event_for_tests(&mut self, event: horizon_browser::RemoteSessionEvent) {
        let mut output = BrowserDrainOutput::default();
        self.apply_remote_session_event(event, &mut output);
    }

    pub(super) fn apply_remote_session_event(
        &mut self,
        event: horizon_browser::RemoteSessionEvent,
        output: &mut BrowserDrainOutput,
    ) {
        use horizon_browser::{RemoteExpiry, RemoteReleaseOutcome, RemoteSessionEvent};
        // Only a positive answer frees the provider's slot: an allocation the
        // provider refused (the driver reports an allocated-then-unreleased
        // session as AllocationUnknown, never as AllocationFailed), or a
        // delete it accepted or no longer knows.
        let released = matches!(
            &event,
            RemoteSessionEvent::AllocationFailed { .. }
                | RemoteSessionEvent::Released {
                    outcome: RemoteReleaseOutcome::Released
                        | RemoteReleaseOutcome::AlreadyGone
                        | RemoteReleaseOutcome::NeverAllocated,
                    ..
                }
                | RemoteSessionEvent::DeviceRejected {
                    released: RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone,
                    ..
                }
        );
        if released && let Some(remote) = self.remote.as_mut() {
            remote.release_established = true;
        }
        let ended = !matches!(
            &event,
            RemoteSessionEvent::Allocating { .. }
                | RemoteSessionEvent::Allocated { .. }
                | RemoteSessionEvent::DeviceIdentity { .. }
        );
        if ended {
            self.clear_remote_identity();
        }
        let mut device = None;
        let mut failure = None;
        let note = match event {
            RemoteSessionEvent::Allocating { label } => format!("allocating remote device for {label}"),
            RemoteSessionEvent::Allocated { label, session_digest } => {
                format!("remote session {session_digest} allocated for {label}")
            }
            RemoteSessionEvent::DeviceIdentity { label, identity } => {
                if let Some(remote) = self.remote.as_mut() {
                    remote.identity_display.confirm(&identity);
                }
                let summary = identity.summary();
                device = Some(summary.clone());
                format!("remote device for {label} verified: {summary}")
            }
            RemoteSessionEvent::DeviceRejected {
                label,
                reason,
                released,
            } => {
                failure = Some(RemoteFailure::DEVICE_REJECTED);
                format!("remote device for {label} rejected: {reason}; session {released}")
            }
            RemoteSessionEvent::AllocationUnknown { label, reason } => {
                failure = Some(RemoteFailure::ALLOCATION_UNKNOWN);
                format!("remote allocation for {label} is unknown ({reason}); check the provider before retrying")
            }
            RemoteSessionEvent::AllocationFailed { label, reason, refusal } => {
                failure = Some(RemoteFailure::for_refusal(refusal));
                format!("remote allocation for {label} failed ({refusal}): {reason}")
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
                RemoteReleaseOutcome::NeverAllocated => format!("remote session for {label} was never allocated"),
            },
        };
        tracing::info!(target: "browser", "{note}");
        self.remote_status = Some(note);
        if let Some(remote) = self.remote.as_mut() {
            if device.is_some() {
                remote.device = device;
            }
            if failure.is_some() {
                remote.failure = failure;
            }
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
        assert!(!state.can_retry(), "a request-less remote panel offers no Retry");

        let backend = state.backend();
        state.switch_backend(BackendKind::FirefoxBidi);
        assert_eq!(
            state.backend(),
            backend,
            "a remote panel's browser is fixed by its target"
        );
        assert!(state.session.is_none());
    }

    #[test]
    fn a_remote_panel_launches_once_and_never_retries() {
        let mut state = BrowserPanelState::inert_remote("ios_phone", "grid");
        assert!(state.is_remote());
        assert!(!state.can_retry(), "Retry would bypass the provider limit");
        assert_eq!(state.remote_provider(), Some("grid"));
        assert!(state.holds_remote_allocation());
        assert!(
            matches!(state.take_remote_request(), Ok(Some(_))),
            "the first launch consumes the request"
        );
        assert!(
            state.take_remote_request().is_err(),
            "nothing is left for a second launch"
        );
        assert_eq!(state.remote_provider(), Some("grid"), "the hold outlives the request");
        assert!(state.holds_remote_allocation());

        state.relaunch();
        assert!(state.session.is_none(), "no session starts from Retry");
        assert!(matches!(state.status, BrowserStatus::Stopped { code: None }));
        assert!(
            state
                .remote_status
                .as_deref()
                .is_some_and(|note| note.contains("create the panel again")),
            "{:?}",
            state.remote_status
        );
        assert!(state.holds_remote_allocation(), "a refused retry never frees the slot");
    }

    #[test]
    fn device_events_record_the_verified_identity_or_a_typed_rejection() {
        use horizon_browser::{DeviceEvidence, RemoteDeviceIdentity, RemoteReleaseOutcome, RemoteSessionEvent};

        let mut state = BrowserPanelState::inert_remote("ios_phone", "grid");
        let mut output = super::super::BrowserDrainOutput::default();
        state.apply_remote_session_event(
            RemoteSessionEvent::DeviceIdentity {
                label: "ios_phone".into(),
                identity: RemoteDeviceIdentity {
                    model: Some("iPhone 16".into()),
                    os_version: Some("18.6".into()),
                    hardware: Some(DeviceEvidence::Physical),
                    ..RemoteDeviceIdentity::default()
                },
            },
            &mut output,
        );
        assert_eq!(state.remote_device(), Some("iPhone 16, OS 18.6, physical device"));
        assert!(state.remote_failure().is_none());
        assert!(state.holds_remote_allocation());

        let mut rejected = BrowserPanelState::inert_remote("ios_phone", "grid");
        rejected.apply_remote_session_event(
            RemoteSessionEvent::DeviceRejected {
                label: "ios_phone".into(),
                reason: "device model is iPhone 15, target requires iPhone 16".into(),
                released: RemoteReleaseOutcome::Released,
            },
            &mut output,
        );
        let failure = rejected.remote_failure().expect("typed failure");
        assert_eq!(failure.code, "remote_device_rejected");
        assert!(
            !failure.message.contains("iPhone"),
            "the public message is fixed text; the provider's detail stays in the panel note"
        );
        assert!(
            rejected
                .remote_status
                .as_deref()
                .is_some_and(|note| note.contains("iPhone 15")),
            "{:?}",
            rejected.remote_status
        );
        assert!(
            !rejected.holds_remote_allocation(),
            "a released rejection frees the slot"
        );

        let mut unreleased = BrowserPanelState::inert_remote("ios_phone", "grid");
        unreleased.apply_remote_session_event(
            RemoteSessionEvent::DeviceRejected {
                label: "ios_phone".into(),
                reason: "target requires a physical device, provider evidence: unverified hardware".into(),
                released: RemoteReleaseOutcome::ReleaseUnknown {
                    attempts: 3,
                    reason: "timeout".into(),
                },
            },
            &mut output,
        );
        assert_eq!(
            unreleased.remote_failure().map(|f| f.code),
            Some("remote_device_rejected")
        );
        assert!(
            unreleased.holds_remote_allocation(),
            "an unconfirmed release keeps the slot"
        );

        let mut failed = BrowserPanelState::inert_remote("ios_phone", "grid");
        failed.apply_remote_session_event(
            RemoteSessionEvent::AllocationFailed {
                label: "ios_phone".into(),
                reason: "no device available".into(),
                refusal: horizon_browser::AllocationRefusal::DeviceUnavailable,
            },
            &mut output,
        );
        assert_eq!(
            failed.remote_failure().map(|f| f.code),
            Some("remote_device_unavailable")
        );
        for (refusal, code) in [
            (
                horizon_browser::AllocationRefusal::Authentication,
                "remote_authentication_failed",
            ),
            (horizon_browser::AllocationRefusal::Entitlement, "remote_not_entitled"),
            (horizon_browser::AllocationRefusal::Other, "remote_allocation_failed"),
        ] {
            let mut panel = BrowserPanelState::inert_remote("ios_phone", "grid");
            panel.apply_remote_session_event(
                RemoteSessionEvent::AllocationFailed {
                    label: "ios_phone".into(),
                    reason: "refused".into(),
                    refusal,
                },
                &mut output,
            );
            assert_eq!(panel.remote_failure().map(|f| f.code), Some(code));
            assert!(!panel.holds_remote_allocation(), "a refusal never holds a slot");
        }
        assert!(!failed.holds_remote_allocation());
    }
}
