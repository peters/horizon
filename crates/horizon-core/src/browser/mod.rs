//! Horizon board state for browser panels.
//!
//! Protocol, process, frame, input, and session ownership live in the
//! UI-independent `horizon-browser` crate. This module adapts that engine to
//! Horizon persistence, panels, and private MCP host coordination.

#[doc(hidden)]
pub mod manifest;
mod orientation;
mod remote_identity;
pub use remote_identity::RemoteIdentityDisplay;
mod attached_host;
pub mod host_driver;
mod host_route;
pub use host_route::{BrowserPanelRequest, BrowserPlace, NewBrowserPanel, new_browser_panel, place_browser_panel};
mod private_profiles;
pub use attached_host::{
    AttachedHost, AttachedHostFailure, AttachedHostFailureKind, DEFAULT_HOST_PROFILE, attached_host_backend_refusal,
};
mod page;
pub use page::panel_title_for_url;
mod status;
pub use status::BrowserStatus;
mod remote_panel;
pub use remote_panel::RemoteFailure;
pub mod file_chooser;
pub mod remote_catalog;
pub mod remote_profile;
pub mod remote_recovery;
pub mod remote_session;
pub use horizon_browser_control::remote_release;
pub use horizon_browser_control::remote_slots;
pub mod remote_usage;
mod shared_session;
pub mod teach;

pub use horizon_browser::remote;
pub use horizon_browser::remote_config;
pub use horizon_browser::{cdp, frames, input, process, session};
use remote::RemoteOrientationView;
pub use remote_session::{RemoteRequestError, browser_family, build_remote_session_request};
pub use teach::{ReviewRow, TeachMode};

use std::path::{Path, PathBuf};

use std::sync::Arc;

pub use horizon_browser::{
    ActiveBackendCapabilities, AllocationRefusal, AutomationDisclosurePolicy, AutomationDisclosureStatus,
    BackendAvailability, BackendCapabilities, BackendKind, BrowserBounds, BrowserButton, BrowserCommand, BrowserConfig,
    BrowserEditCommand, BrowserEvent, BrowserEventWaker, BrowserInput, BrowserKey, BrowserModifiers, BrowserSession,
    BrowserShutdownSignal, BrowserVideoCapture, BrowserVideoCaptureOptions, BrowserVideoOperation, BrowserVideoState,
    DEFAULT_VIDEO_COMPRESSION_LEVEL, DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_MAX_FILE_BYTES, DEFAULT_VIDEO_QUALITY,
    DEFAULT_VIEWPORT, FrameDelivery, FrameMetrics, FrameSlot, MAX_VIDEO_COMPRESSION_LEVEL, MAX_VIDEO_FILE_BYTES,
    MAX_VIDEO_FPS, MAX_VIDEO_MAX_WIDTH, MAX_VIDEO_QUALITY, MIN_VIDEO_FILE_BYTES, MIN_VIDEO_FPS, MIN_VIDEO_MAX_WIDTH,
    MIN_VIDEO_QUALITY, NativeSelectOption, NativeSelectPopup, NestedScrollbar, PageScrollState, RemoteAllocation,
    RemoteRecoveryStatus, RemoteReleaseOutcome, RemoteSessionEvent, RemoteSessionRequest, normalize_navigation_target,
};
const FORCED_CHROME_SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Panel content for `PanelKind::Browser` panels.
pub struct BrowserPanelState {
    pub status: BrowserStatus,
    /// Last URL Chrome actually committed. `Some("")` is a committed blank
    /// target; `None` means the requested startup navigation is still only a
    /// request and must not enter persistence.
    pub url: Option<String>,
    pub title: String,
    pub loading: bool,
    pub frame_slot: Arc<FrameSlot>,
    session: Option<Box<BrowserSession>>,
    /// Outlives the session handle so a final driver-side navigation can be
    /// folded into persistence after browser teardown completes.
    committed_url: session::CommittedUrl,
    /// Driver teardown that must finish before Retry or application exit can
    /// reuse/release this panel's Chrome profile.
    teardown_signal: Option<Box<BrowserShutdownSignal>>,
    /// One-click Retry waits for the prior Chrome profile lock to be released,
    /// then starts this replacement automatically.
    pending_relaunch: Option<PendingRelaunch>,
    pub panel_local_id: String,
    /// Initial URL requested at (re)start, for the URL bar.
    pub requested_url: Option<String>,
    /// Agent currently driving this panel (from manifest heartbeats).
    pub owner: Option<String>,
    /// Pending handoff request from the agent, with its reason.
    pub handoff_reason: Option<String>,
    /// Manifest failure from the most recent hand-back attempt.
    pub handoff_error: Option<String>,
    /// The driver is persisting the user's hand-back acknowledgement.
    pub handoff_resolution_pending: bool,
    /// Latest page selection copied by Chrome and not yet written to the host
    /// clipboard by the UI.
    pending_clipboard_text: Option<String>,
    host_focus_request: Option<bool>,
    /// Most recent URL submission error; cleared after a committed navigation.
    pub navigation_error: Option<String>,
    /// Last page-pixel recording failure; cleared when a new recording starts.
    pub video_error: Option<String>,
    /// Latest remote-session lifecycle note (allocation, expiry, release), value-free.
    pub remote_status: Option<String>,
    pub orientation: RemoteOrientationView,
    orientation_pending_since: Option<std::time::Instant>,
    orientation_queue_rejection: Option<String>,
    /// The remote session this panel runs (or ran) at; `None` for a local
    /// browser. Carries the resolved authorization for this process only.
    remote: Option<remote_panel::RemoteLifecycle>,
    /// The machine that runs the Chromium of this panel, with the forward that
    /// carries its `DevTools` port; `None` for a browser of this computer.
    attached: Option<attached_host::AttachedHostSlot>,
    /// The link to the driver that runs beside the Firefox of a browser host;
    /// `None` for a panel of this computer and for a Chromium host.
    host_driver: Option<host_driver::panel::HostDriverSlot>,
    /// The endpoint of the connect that the next session drives. It exists only
    /// for the moment between a finished connect and the session of that
    /// browser.
    attach_endpoint: Option<horizon_browser::DevtoolsEndpoint>,
    /// The page target of the browser host that this panel drives, as the
    /// driver reported it. A lost link passes it back as the resume target, so
    /// the same page comes back with the text the person typed into it.
    attached_page_target: Option<String>,
    /// The isolated browser context of a private page of a browser host, as the
    /// driver reported it. A lost link passes it back with the page target, so
    /// the page comes back inside its own context and never as a page of the
    /// shared profile.
    attached_page_context: Option<String>,
    /// What the session of this panel established about its page: a page of an
    /// isolated context, a page of a folder of this panel alone, or a page of
    /// the profile that it named. The label of the panel follows this fact.
    page_isolation: horizon_browser::PageIsolationReport,
    /// What the driver of a browser host established about the page that it
    /// drives, for the Firefox of that machine, whose page this computer reads
    /// through the link of its driver rather than through a session of its own.
    host_page_keeps_nothing: bool,
    /// The link came back to the same page: the body says so and no page state
    /// was lost.
    pub page_resumed: bool,
    /// When the panel started to say that a lost link came back to the same page.
    resumed_notice_at: Option<std::time::Instant>,
    /// User-typed navigation kept as the display and retry target until the
    /// driver commits a reachable page, so the input is never discarded.
    pending_user_navigation: Option<String>,
    /// User activity that can navigate (typed URL, back, forward, reload, a
    /// click, a key press) so far; lets a pending create tell a user takeover
    /// from the requested first page committing.
    user_navigations: std::sync::atomic::AtomicU32,
    /// A backend selection changed since the last output drain and must be
    /// folded into the persisted runtime state exactly once.
    persisted_config_changed: bool,
    config: BrowserConfig,
    shared_session: Option<Arc<shared_session::ProfileSession>>,
    teach: Option<TeachMode>,
}

struct PendingRelaunch {
    initial_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserDrainOutput {
    pub had_output: bool,
    pub url_changed: bool,
    pub config_changed: bool,
}

impl BrowserPanelState {
    /// Whether the session has ended, rather than reporting a warning while
    /// its driver is still recovering or waiting to relaunch. A panel of a
    /// browser host that lost its browser after it was ready has not ended: it
    /// waits for the person's Retry with its last address.
    #[must_use]
    pub fn has_ended(&self) -> bool {
        !self.attached_kept_after()
            && !self.host_driver_kept_after()
            && !self.status.is_alive()
            && !self.status.is_reconnecting()
            && self.session.is_none()
            && self.pending_relaunch.is_none()
    }

    /// Driver-less panel for tests that only exercise command dispatch.
    #[doc(hidden)]
    #[must_use]
    pub fn inert() -> Self {
        Self {
            status: BrowserStatus::Stopped { code: None },
            url: None,
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "inert".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            shared_session: None,
            teach: None,
        }
    }

    /// Create the state and start the driver thread.
    ///
    /// # Errors
    ///
    /// Returns an error if a relative profile root cannot be resolved from
    /// the process launch directory.
    pub fn start(
        panel_local_id: impl Into<String>,
        config: &BrowserConfig,
        initial_url: Option<String>,
    ) -> crate::error::Result<Self> {
        Self::start_with_session(panel_local_id.into(), config, initial_url, None)
    }

    /// Restore a profile group or create an isolated browser panel.
    ///
    /// `session_id` is the name of the shared profile of the panel, or `None`
    /// for a private panel whose folder goes away at its close.
    ///
    /// # Errors
    /// Invalid launch paths cannot be resolved, or the browser of the named
    /// profile cannot take this page.
    pub fn start_with_session(
        panel_local_id: String,
        config: &BrowserConfig,
        initial_url: Option<String>,
        session_id: Option<&str>,
    ) -> crate::error::Result<Self> {
        let mut config = config.resolved_for_launch()?;
        let home = crate::horizon_home::HorizonHome::resolve();
        let profile_root_resolved = retain_effective_profile_root(&mut config, &home.root().join("browser-profiles"));
        // The profile of this page decides where its folder is: a shared profile
        // under the profile root, and a page that keeps nothing under the folder
        // of the private pages. The saved board names the shared profile, and a
        // panel without one is a private panel.
        config.profile = Some(session_id.unwrap_or(horizon_browser::PRIVATE_PROFILE).to_string());
        let initial_url = initial_url.map(|url| normalize_navigation_target(&url));
        if let Some(profile_id) = session_id {
            shared_session::refuse_unjoinable(&config, profile_id)?;
        }
        let shared_session = matches!(config.backend, BackendKind::ChromiumCdp | BackendKind::FirefoxBidi)
            .then(|| shared_session::acquire(&config, session_id.unwrap_or(&panel_local_id)));
        if session_id.is_none() {
            // A private panel of this computer keeps its own throwaway folder.
            // The mark says that this program made it for such a panel and that
            // the folder goes away when no saved session names it, so a run that
            // ends without a close leaves nothing on the disk for ever.
            let folder = profile_dir_for_home(&config, &home, &panel_local_id);
            if let Err(error) = private_profiles::mark_private_profile(&folder) {
                tracing::warn!(%error, folder = %folder.display(), "the folder of a private browser panel carries no mark");
            }
        }
        let mut state = Self {
            status: BrowserStatus::Starting,
            url: None,
            title: String::new(),
            loading: true,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            requested_url: initial_url.clone(),
            panel_local_id,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: profile_root_resolved,
            config,
            shared_session,
            teach: None,
        };
        state.launch_session(initial_url);
        Ok(state)
    }

    /// (Re)start the driver — used for the Retry action after a failure.
    /// Resumes at the last-viewed URL when one is known, so a crashed
    /// browser returns the user to where they were rather than the panel's
    /// initial URL.
    pub fn relaunch(&mut self) {
        if self.is_host_driver() {
            self.relaunch_host_driver();
            return;
        }
        if self.is_attached() {
            self.relaunch_attached();
            return;
        }
        self.launch_session(self.relaunch_target());
    }

    /// The URL the next relaunch should open: a navigation the user typed
    /// while the driver was absent wins over the last committed URL, which
    /// wins over the requested startup target.
    fn relaunch_target(&self) -> Option<String> {
        self.pending_user_navigation
            .clone()
            .or_else(|| self.url.clone().filter(|url| !url.is_empty()))
            .or_else(|| self.requested_url.clone().filter(|url| !url.is_empty()))
    }

    /// URL shown in browser chrome. A navigation typed while the driver was
    /// absent stays visible as the pending relaunch target; before Chrome
    /// commits anything, preserve the requested startup target as editable
    /// UI without treating it as persisted browser history.
    #[must_use]
    pub fn display_url(&self) -> &str {
        if let Some(pending) = self.pending_user_navigation.as_deref().filter(|url| !url.is_empty()) {
            return pending;
        }
        self.url.as_deref().unwrap_or_else(|| {
            self.requested_url
                .as_deref()
                .filter(|url| *url != "about:blank")
                .unwrap_or_default()
        })
    }

    /// Last URL Chrome actually committed. `Some("")` means Chrome committed
    /// a blank target; `None` means the requested startup navigation has not
    /// committed and must not enter runtime persistence yet.
    #[must_use]
    pub fn committed_url_for_persistence(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Profile root this panel was launched with. Persisting it separately
    /// keeps the profile reachable if the live config changes before the
    /// panel is next saved.
    #[must_use]
    pub fn profile_root_for_persistence(&self) -> Option<&Path> {
        self.config.profile_root.as_deref()
    }

    #[must_use]
    pub const fn backend(&self) -> BackendKind {
        self.config.backend
    }

    #[must_use]
    pub fn backend_capabilities(&self) -> BackendCapabilities {
        if self.is_remote() {
            BackendCapabilities::remote_session()
        } else {
            self.config.backend.capabilities()
        }
    }

    #[must_use]
    pub fn active_backend_capabilities(&self) -> Option<ActiveBackendCapabilities> {
        self.frame_slot.active_backend_capabilities()
    }

    /// Stop the exact current driver and queue a replacement using another
    /// backend. The replacement starts only after the old process has been
    /// reaped, so panel switching cannot overlap profile or port ownership.
    pub fn switch_backend(&mut self, backend: BackendKind) {
        if self.teach.is_some() || self.config.backend == backend || self.is_remote() || self.is_attached() {
            // A remote session's browser is fixed by its target, and the engine
            // of a page of a browser host is fixed by the profile of that host.
            return;
        }
        if self.shared_session.as_ref().is_some_and(|session| {
            shared_session::has_shared_members(&self.config, session) || session.profile_id() != self.panel_local_id
        }) {
            self.navigation_error = Some("Shared browser panels must keep the same backend".into());
            return;
        }
        let target = self.relaunch_target();
        self.config.backend = backend;
        self.shared_session = if matches!(backend, BackendKind::ChromiumCdp | BackendKind::FirefoxBidi) {
            Some(shared_session::acquire(&self.config, &self.panel_local_id))
        } else {
            None
        };
        self.persisted_config_changed = true;
        self.frame_slot.clear();
        self.frame_slot.clear_backend_capabilities();
        self.launch_session(target);
    }

    fn launch_session(&mut self, initial_url: Option<String>) {
        self.reset_orientation_view();
        self.navigation_error = None;
        let Ok(remote) = self.take_remote_request() else {
            self.refuse_remote_relaunch();
            return;
        };
        self.frame_slot.clear_backend_capabilities();
        // Drop the previous driver (if any) and wait for its Chrome to be
        // gone: the replacement reuses the same profile directory, and a
        // second instance would lose the profile lock race.
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new(BrowserSession::shutdown_signal(*session)));
        }
        if !self.retry_ready() {
            debug_assert!(remote.is_none(), "a remote panel has no previous driver to wait for");
            self.pending_relaunch = Some(PendingRelaunch { initial_url });
            self.loading = true;
            self.status = BrowserStatus::Starting;
            return;
        }
        self.start_session(initial_url, remote);
    }

    fn start_session(&mut self, initial_url: Option<String>, remote: Option<RemoteSessionRequest>) {
        // A fact of the link before never describes the page of the link now: a
        // new session proves its own isolation, and a page that nothing proves
        // is never shown as private.
        self.page_isolation = horizon_browser::PageIsolationReport::default();
        let browser = self.config.clone();
        let home = crate::horizon_home::HorizonHome::resolve();
        let capture_directory = self.capture_directory(&home);
        self.frame_slot.file_chooser().register_consumer();
        let session_config = session::BrowserSessionConfig {
            attach: self.take_attach_endpoint(),
            // Only a panel that drives a browser of another computer can adopt
            // the page it had: that browser kept running behind the link.
            resume_target: self.attached_page_target.clone().filter(|_| self.is_attached()),
            // A private page comes back inside the context that it had, or not
            // at all: the panel never resumes a page of the default context.
            resume_context: self.attached_page_context.clone().filter(|_| self.is_attached()),
            page_isolation: self.page_isolation.clone(),
            browser,
            panel_local_id: self.panel_local_id.clone(),
            // A navigation typed while the driver was absent takes precedence
            // over the queued relaunch target: it is the freshest request.
            initial_url: self.pending_user_navigation.clone().or(initial_url),
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
            // Reuse the panel's slot across (re)starts: frame sequence
            // numbers stay monotonic, so a retried session's first frame is
            // never mistaken for an unchanged one by the UI's seq check.
            frame_slot: Arc::clone(&self.frame_slot),
            coordination: Some(Arc::new(manifest::ManifestCoordination::with_remote_allocation(
                remote.as_ref().map(|request| request.recovery.clone()),
            ))),
            capture_directory: Some(capture_directory),
            video: Arc::new(horizon_browser::VideoCaptureHandle::default()),
            remote,
        };
        let recovery = session_config.remote.as_ref().map(|request| request.recovery.clone());
        let started = if let Some(group) = &self.shared_session {
            session::start_shared_session(session_config, group.as_ref().clone())
        } else {
            session::start_session(session_config)
        };
        match started {
            Ok(handle) => {
                self.committed_url = handle.committed_url();
                // The pending submission is only cleared once the session
                // commits a navigation (see sync_committed_url): a startup
                // that dies before navigate_to must keep it so Retry
                // re-targets the user's request, not the stale URL.
                self.clear_agent_state_for_relaunch();
                self.status = BrowserStatus::Starting;
                self.loading = true;
                self.session = Some(Box::new(handle));
            }
            Err(error) => {
                if let Some(recovery) = recovery {
                    recovery.cancel_before_launch();
                }
                self.status = BrowserStatus::Error {
                    message: error.to_string(),
                };
            }
        }
    }

    fn continue_pending_relaunch(&mut self) -> bool {
        if self.pending_relaunch.is_none() || !self.retry_ready() {
            return false;
        }
        let Some(pending) = self.pending_relaunch.take() else {
            return false;
        };
        // A queued relaunch is always local: a remote panel launches once.
        self.start_session(pending.initial_url, None);
        true
    }

    fn clear_agent_state_for_relaunch(&mut self) {
        self.owner = None;
        self.handoff_reason = None;
        self.handoff_error = None;
        self.handoff_resolution_pending = false;
    }

    pub fn send(&self, command: BrowserCommand) {
        let _ = self.try_send(command);
    }

    #[must_use]
    pub fn video_capture(&self) -> Option<BrowserVideoCapture> {
        self.session.as_ref().and_then(|session| session.video_capture())
    }

    #[must_use]
    pub fn needs_event_waker(&self) -> bool {
        if self.is_host_driver() {
            return self.host_driver_waker_needed();
        }
        if self.is_attached() {
            return self.needs_attached_waker();
        }
        self.session.as_ref().is_some_and(|session| session.needs_event_waker())
    }

    pub fn set_event_waker(&self, callback: BrowserEventWaker) {
        if self.is_host_driver() {
            self.set_host_driver_waker(callback);
            return;
        }
        if self.is_attached() {
            self.set_attached_waker(callback);
            return;
        }
        if let Some(session) = &self.session {
            session.set_event_waker(callback);
        }
    }

    /// Live local driver or remote channel. A handle can outlive its receiver.
    #[must_use]
    pub fn can_accept_commands(&self) -> bool {
        if self.is_host_driver() {
            return self.host_driver_takes_commands();
        }
        self.session.as_deref().is_some_and(BrowserSession::can_accept_commands)
    }

    /// Chrome record control. Remote video status never returns to this panel.
    #[must_use]
    pub fn can_start_recording(&self) -> bool {
        self.can_accept_commands()
    }

    /// Queue a command only when the driver session currently exists.
    #[must_use]
    pub fn try_send(&self, command: BrowserCommand) -> bool {
        // Chrome commands, and the input that can navigate from inside the
        // page (a click on a link, Enter in a form), count as user activity
        // that may take a pending startup navigation over, but only once the
        // driver accepted them; pointer moves and wheel events do not
        // navigate on their own.
        let may_navigate = page::command_may_navigate(&command);
        let accepted = if self.is_host_driver() {
            self.send_host_driver_command(command)
        } else {
            self.session.as_ref().is_some_and(|session| session.send(command))
        };
        if accepted && may_navigate {
            self.user_navigations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        accepted
    }

    /// Number of user actions that can navigate (typed URL, back, forward,
    /// reload, click, key press) on this panel so far.
    #[must_use]
    pub fn user_navigation_count(&self) -> u32 {
        self.user_navigations.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Submit a user-typed navigation and retain it as the display/retry
    /// target until a reachable page commits. Returns `true` when the driver
    /// accepted it; a missing driver is surfaced via `navigation_error`.
    pub fn submit_navigation(&mut self, url: &str) -> bool {
        let url = normalize_navigation_target(url);
        self.pending_user_navigation = Some(url.clone());
        self.navigation_error = None;
        if self.try_send(BrowserCommand::Navigate(url)) {
            return true;
        }
        self.navigation_error = Some("Browser is not running; press Retry to open the typed URL".to_string());
        false
    }

    /// Take page text copied by headless Chrome for the UI's host clipboard.
    pub fn take_clipboard_text(&mut self) -> Option<String> {
        self.pending_clipboard_text.take()
    }

    /// Take a Safari request to restore focus to the host viewport.
    pub fn take_host_focus_request(&mut self) -> Option<bool> {
        self.host_focus_request.take()
    }

    /// Tell the driver the user handed the panel back to the agent.
    pub fn hand_back(&mut self) {
        self.handoff_error = None;
        if self.try_send(BrowserCommand::HandoffDone) {
            self.handoff_resolution_pending = true;
        } else {
            self.handoff_resolution_pending = false;
            self.handoff_error = Some("browser driver unavailable; retry after recovery".to_string());
        }
    }

    /// Stop the driver asynchronously (panel close during a session):
    /// the driver finishes its Chrome teardown on its own thread. The
    /// teardown-completion signal is preserved (as in
    /// [`Self::request_shutdown`]) so a live state keeps process control
    /// over the stopping driver and undrained frame notifications are
    /// released.
    pub fn stop(&mut self) {
        self.shutdown_attached();
        self.shutdown_host_driver();
        self.clear_remote_identity();
        // A queued Retry must not survive an explicit stop: drain_events
        // would otherwise relaunch Chrome after the caller stopped it.
        self.pending_relaunch = None;
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new((*session).shutdown_signal()));
        }
        if matches!(self.status, BrowserStatus::Starting | BrowserStatus::Ready) {
            self.status = BrowserStatus::Stopped { code: None };
        }
    }

    /// Ask the driver to stop and remember its teardown-completion signal;
    /// the app-shutdown paths join on it so exit cannot outrun the profile
    /// lock.
    pub fn request_shutdown(&mut self) {
        self.shutdown_attached();
        self.shutdown_host_driver();
        self.clear_remote_identity();
        self.pending_relaunch = None;
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new((*session).shutdown_signal()));
        }
        if matches!(self.status, BrowserStatus::Starting | BrowserStatus::Ready) {
            self.status = BrowserStatus::Stopped { code: None };
        }
    }

    /// Take the stored teardown-completion signal, if a shutdown was
    /// requested and not yet joined.
    pub(crate) fn take_shutdown_signal(&mut self) -> Option<BrowserShutdownSignal> {
        self.teardown_signal.take().map(|signal| *signal)
    }

    /// Request driver shutdown and wait up to `timeout` for Chrome teardown.
    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: std::time::Duration) -> bool {
        self.request_shutdown();
        self.take_shutdown_signal()
            .is_none_or(|signal| signal.wait(timeout) || signal.force_cleanup(FORCED_CHROME_SHUTDOWN_WAIT))
    }

    /// Whether a failed/stopped session has fully released its Chrome
    /// process and profile, making Retry safe. Polling is non-blocking so the
    /// recovery placeholder cannot freeze the UI.
    pub fn retry_ready(&mut self) -> bool {
        if self.is_host_driver() {
            // The browser of the other machine owns its own profile, so a Retry
            // starts a link and waits for no local teardown.
            return true;
        }
        let Some(signal) = &self.teardown_signal else {
            return true;
        };
        if signal.is_complete() {
            self.teardown_signal = None;
            true
        } else {
            false
        }
    }

    /// Drain driver events into panel state, separating visible activity from
    /// URL changes that must dirty the persisted runtime state.
    pub fn drain_events(&mut self) -> BrowserDrainOutput {
        let orientation_expired = self.expire_orientation_pending();
        let notice_expired = self.expire_resumed_notice_at(std::time::Instant::now());
        if self.poll_host_driver() {
            return BrowserDrainOutput {
                had_output: true,
                ..BrowserDrainOutput::default()
            };
        }
        if self.poll_attached() {
            return BrowserDrainOutput {
                had_output: true,
                ..BrowserDrainOutput::default()
            };
        }
        let relaunched = self.continue_pending_relaunch();
        let config_changed = std::mem::take(&mut self.persisted_config_changed);
        let events = self
            .session
            .as_ref()
            .map(|session| session.event_rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut output = BrowserDrainOutput {
            had_output: relaunched || config_changed || orientation_expired || notice_expired,
            config_changed,
            ..BrowserDrainOutput::default()
        };
        for event in events {
            self.apply_event(event, &mut output);
        }
        self.sync_committed_url(&mut output);
        self.drain_teach_observations(&mut output);
        output
    }

    fn drain_teach_observations(&mut self, output: &mut BrowserDrainOutput) {
        let Some(teach) = self.teach.as_mut() else {
            return;
        };
        let mut ingested = false;
        while let Some(observation) = self.frame_slot.take_teach_observation() {
            teach.ingest(observation);
            ingested = true;
        }
        if ingested {
            output.had_output = true;
        }
    }

    /// Start Teach recording on this panel.
    ///
    /// # Errors
    /// Private routine storage could not be created.
    pub fn start_teach(&mut self, name: impl Into<String>) -> Result<(), horizon_browser_routines::RoutineError> {
        if self.teach.as_ref().is_some_and(TeachMode::is_stopped) {
            return Ok(());
        }
        if self.teach.is_some() {
            self.resume_teach();
            return Ok(());
        }
        let teach = TeachMode::start(name, self.backend())?;
        self.frame_slot.set_teach_recording(true);
        self.teach = Some(teach);
        Ok(())
    }

    #[must_use]
    pub fn teach(&self) -> Option<&TeachMode> {
        self.teach.as_ref()
    }

    pub fn teach_mut(&mut self) -> Option<&mut TeachMode> {
        self.teach.as_mut()
    }

    pub fn pause_teach(&mut self) {
        if let Some(teach) = self.teach.as_mut() {
            teach.pause();
        }
        self.frame_slot.set_teach_recording(false);
    }

    pub fn resume_teach(&mut self) {
        if self.handoff_reason.is_some() {
            return;
        }
        if let Some(teach) = self.teach.as_mut() {
            teach.resume();
        }
        if self
            .teach
            .as_ref()
            .is_some_and(|teach| !teach.is_stopped() && !teach.is_paused())
        {
            self.frame_slot.set_teach_recording(true);
        }
    }

    pub fn stop_teach(&mut self) {
        if let Some(teach) = self.teach.as_mut() {
            teach.stop();
        }
        self.frame_slot.set_teach_recording(false);
    }

    /// Discard the Teach session and its draft.
    ///
    /// # Errors
    /// Draft unlink failure.
    pub fn discard_teach(&mut self) -> Result<(), horizon_browser_routines::RoutineError> {
        self.frame_slot.set_teach_recording(false);
        if let Some(teach) = self.teach.as_mut() {
            teach.discard()?;
        }
        self.teach = None;
        Ok(())
    }

    fn apply_event(&mut self, event: BrowserEvent, output: &mut BrowserDrainOutput) {
        match event {
            BrowserEvent::BackendReady(_) => output.had_output = true,
            BrowserEvent::Ready => {
                self.status = BrowserStatus::Ready;
                // A startup without a fresh navigation never receives
                // Loading(false); a real navigation queues Loading(true)
                // immediately after Ready.
                self.loading = false;
                self.attached_reached_ready();
                output.had_output = true;
            }
            BrowserEvent::Title(title) => {
                if self.title != title {
                    self.title = title;
                    output.had_output = true;
                }
            }
            BrowserEvent::UrlChanged(url) => self.apply_committed_url(&url, output),
            BrowserEvent::NavigationFailed(message) => {
                self.navigation_error = Some(message);
                output.had_output = true;
            }
            BrowserEvent::Loading(loading) => self.loading = loading,
            BrowserEvent::Frame { .. } => {
                self.frame_slot.release_notification();
                output.had_output = true;
            }
            BrowserEvent::Warning(message) => {
                self.frame_slot.clear();
                self.status = BrowserStatus::Error { message };
                output.had_output = true;
            }
            BrowserEvent::PageAttached {
                target_id,
                resumed,
                context,
            } => {
                self.attached_page_target = Some(target_id);
                self.attached_page_context = context;
                self.apply_page_attached(resumed);
                output.had_output = true;
            }
            BrowserEvent::OrientationChanged(view) => {
                self.apply_orientation_view(view);
                output.had_output = true;
            }
            BrowserEvent::VideoFailed(message) => {
                self.video_error = Some(message);
                output.had_output = true;
            }
            BrowserEvent::Stopped { code } => self.apply_stopped(code, output),
            BrowserEvent::RemoteSession(event) => self.apply_remote_session_event(event, output),
            BrowserEvent::HandoffRequested(reason) => {
                self.handoff_reason = Some(reason);
                self.handoff_error = None;
                self.handoff_resolution_pending = false;
                if self.teach.is_some() {
                    self.pause_teach();
                }
                output.had_output = true;
            }
            BrowserEvent::HandoffCleared => {
                self.handoff_error = None;
                self.handoff_resolution_pending = false;
                if self.handoff_reason.is_some() {
                    self.handoff_reason = None;
                    output.had_output = true;
                }
            }
            BrowserEvent::HandoffResolutionFailed(message) => {
                self.handoff_error = Some(message);
                self.handoff_resolution_pending = false;
                output.had_output = true;
            }
            BrowserEvent::ClipboardText(text) => {
                self.pending_clipboard_text = Some(text);
                output.had_output = true;
            }
            BrowserEvent::HostFocusRequested { ready } => {
                self.host_focus_request = Some(self.host_focus_request.unwrap_or(false) || ready);
                output.had_output = true;
            }
            BrowserEvent::OwnerChanged(owner) => {
                if self.owner != owner {
                    self.owner = owner;
                    output.had_output = true;
                }
            }
        }
    }

    fn apply_committed_url(&mut self, url: &str, output: &mut BrowserDrainOutput) {
        if self.url.as_deref() != Some(url) {
            self.url = Some(url.to_string());
            output.url_changed = true;
        }
        self.navigation_error = None;
        // The event sender publishes this URL before queuing the event, so
        // sync_committed_url cannot reliably clear a pending submission.
        if !url.is_empty() && url != "about:blank" {
            self.pending_user_navigation = None;
        }
        output.had_output = true;
    }

    fn apply_stopped(&mut self, code: Option<i32>, output: &mut BrowserDrainOutput) {
        self.release_attached_forward();
        self.reset_orientation_view();
        self.clear_remote_identity();
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new((*session).completion_signal()));
        }
        self.frame_slot.clear();
        let keep_error = matches!(self.status, BrowserStatus::Error { .. });
        if !keep_error || code.is_some() {
            self.status = BrowserStatus::Stopped { code };
        }
        // A panel that showed the page of a browser host keeps its reason and
        // its last address: the browser of that machine stopped answering, and
        // only Retry replaces the session.
        if let BrowserStatus::Error { message } = &self.status {
            let reason = message.clone();
            self.attached_lost_after_ready(&reason);
        }
        if self.handoff_resolution_pending {
            self.handoff_resolution_pending = false;
            self.handoff_error = Some("browser stopped before the hand-back acknowledgement was saved".to_string());
        }
        output.had_output = true;
    }

    fn sync_committed_url(&mut self, output: &mut BrowserDrainOutput) {
        let Some(url) = self.committed_url.snapshot() else {
            return;
        };
        if self.url.as_deref() == Some(url.as_str()) {
            return;
        }
        // The driver committed a real navigation: the pending submission it
        // was carrying is done (exact match or the post-redirect
        // destination). about:blank commits are not navigation results.
        if url != "about:blank" {
            self.pending_user_navigation = None;
        }
        // A page that navigated no longer reports the page that came back after
        // a lost link; the person is looking at a new document.
        self.set_page_resumed(false);
        self.url = Some(url);
        self.navigation_error = None;
        output.url_changed = true;
        output.had_output = true;
    }
}

impl Drop for BrowserPanelState {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Resolve the private host-coordination directory for browser panels.
#[doc(hidden)]
#[must_use]
pub fn manifest_dir() -> PathBuf {
    manifest::default_manifest_dir()
}

pub(crate) fn profile_dir_for_home(
    config: &BrowserConfig,
    home: &crate::horizon_home::HorizonHome,
    panel_local_id: &str,
) -> PathBuf {
    config.panel_profile_dir_with_default_root(panel_local_id, &home.root().join("browser-profiles"))
}

fn retain_effective_profile_root(config: &mut BrowserConfig, default_root: &Path) -> bool {
    if config.profile_root.is_some() {
        return false;
    }
    config.profile_root = Some(config.effective_profile_root(default_root));
    true
}

pub use private_profiles::{PrivateProfileSweep, is_private_profile, mark_private_profile, sweep_private_profiles};

pub use horizon_browser::provider_catalog;
pub use horizon_browser::{PageIsolation, private_page_unavailable_detail};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = BrowserConfig::default();
        assert_eq!(config.quality, 60);
        assert_eq!(config.every_nth_frame, 1);
        assert!(config.command.is_none());
    }

    /// A run with a Horizon home of its own keeps every folder that a browser
    /// panel makes when it starts under that home, so a second Horizon never
    /// writes into the profiles of the first one.
    #[test]
    fn a_private_horizon_home_holds_the_folders_of_a_browser_panel() {
        let private = std::path::PathBuf::from("/private/horizon-home");
        let home = crate::horizon_home::HorizonHome::from_root(private.clone());
        let config = BrowserConfig::default();
        let profile = profile_dir_for_home(&config, &home, "panel-1");

        assert_eq!(
            profile,
            private
                .join("browser-profiles")
                .join(horizon_browser_control::paths::safe_local_id("panel-1")),
            "the folder of a panel is under the home of its run"
        );
        assert!(
            profile.join("captures").starts_with(&private),
            "the folder of a capture is under the same home"
        );
        // The profile of the engine is the folder of the panel and the folder of
        // its backend, which is what a launch creates.
        let launched = config.panel_profile_dir_with_default_root("panel-1", &private.join("browser-profiles"));
        assert_eq!(launched, profile, "the engine and the panel name one folder");
    }

    #[test]
    fn resolved_profile_root_is_retained_for_persistence_and_cleanup() {
        let mut config = BrowserConfig::default();

        assert!(retain_effective_profile_root(
            &mut config,
            Path::new("/profiles/first-default")
        ));
        assert_eq!(config.profile_root, Some(PathBuf::from("/profiles/first-default")));
        assert!(!retain_effective_profile_root(
            &mut config,
            Path::new("/profiles/later-default")
        ));
        assert_eq!(config.profile_root, Some(PathBuf::from("/profiles/first-default")));
    }

    #[test]
    fn status_transitions() {
        assert!(BrowserStatus::default().is_alive());
        assert!(BrowserStatus::Ready.is_alive());
        assert!(!BrowserStatus::Stopped { code: None }.is_alive());
        assert!(
            !BrowserStatus::Error {
                message: "x".to_string()
            }
            .is_alive()
        );
    }

    #[test]
    fn warning_during_recovery_is_not_an_ended_session() {
        let root = tempfile::tempdir().expect("profile root");
        // This test needs a launch that fails at once. A program of this computer
        // that ends at once means that; a path that this computer does not have
        // would fall back to the browser that the computer happens to have.
        let config = BrowserConfig {
            command: Some(crate::util::stand_in_program().to_string()),
            profile_root: Some(root.path().join("profiles")),
            ..BrowserConfig::default()
        };
        let mut state = BrowserPanelState::start("warning-recovery", &config, None).expect("session handle");
        let mut output = BrowserDrainOutput::default();
        state.apply_event(BrowserEvent::Warning("temporary setup failure".into()), &mut output);
        assert!(!state.has_ended(), "a warning cannot retire an attached driver");
        state.apply_event(BrowserEvent::Ready, &mut output);
        assert!(!state.has_ended());
        state.apply_event(BrowserEvent::Warning("terminal failure".into()), &mut output);
        state.apply_event(BrowserEvent::Stopped { code: None }, &mut output);
        assert!(state.has_ended());
        assert!(matches!(state.status, BrowserStatus::Error { .. }));
        assert!(state.shutdown_with_timeout(std::time::Duration::from_secs(5)));
    }

    #[test]
    fn submission_without_a_driver_is_retained_and_surfaced() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: Some("https://old.example/".to_string()),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "submit-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        assert!(!state.submit_navigation("typed.example/page"));
        assert!(state.navigation_error.is_some());
        // The typed URL stays visible in the URL bar even though the last
        // committed URL is known, and becomes the relaunch target.
        assert_eq!(state.display_url(), "https://typed.example/page");
        assert_eq!(state.relaunch_target(), Some("https://typed.example/page".to_string()));
        // Persistence semantics are untouched: only committed URLs persist.
        assert_eq!(state.committed_url_for_persistence(), Some("https://old.example/"));
    }

    #[test]
    fn retry_stays_disabled_until_teardown_completes() {
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let mut state = BrowserPanelState {
            status: BrowserStatus::Error {
                message: "stopped".to_string(),
            },
            url: None,
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: Some(Box::new(BrowserShutdownSignal::for_test(completion_rx))),
            pending_relaunch: None,
            panel_local_id: "retry-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        assert!(!state.retry_ready());
        assert_eq!(completion_tx.send(()), Ok(()));
        assert!(state.retry_ready());
        assert!(state.teardown_signal.is_none());
    }

    #[test]
    fn backend_switch_marks_persisted_state_dirty_once() {
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let mut state = BrowserPanelState {
            status: BrowserStatus::Ready,
            url: Some("https://example.com/".to_string()),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: Some(Box::new(BrowserShutdownSignal::for_test(completion_rx))),
            pending_relaunch: None,
            panel_local_id: "backend-persistence-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        state.switch_backend(BackendKind::FirefoxBidi);
        assert_eq!(state.backend(), BackendKind::FirefoxBidi);

        let first = state.drain_events();
        assert!(first.config_changed);
        assert!(first.had_output);
        assert!(!first.url_changed);

        let second = state.drain_events();
        assert!(!second.config_changed);
        assert_eq!(completion_tx.send(()), Ok(()));
    }

    #[test]
    fn shutdown_with_timeout_waits_for_driver_completion() {
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let (start_tx, start_rx) = std::sync::mpsc::channel();
        let mut state = BrowserPanelState {
            status: BrowserStatus::Ready,
            url: None,
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: Some(Box::new(BrowserShutdownSignal::for_test(completion_rx))),
            pending_relaunch: None,
            panel_local_id: "shutdown-wait-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };
        let completion = std::thread::spawn(move || {
            let _ = start_rx.recv();
            std::thread::sleep(std::time::Duration::from_millis(30));
            completion_tx.send(())
        });
        let started = std::time::Instant::now();
        assert_eq!(start_tx.send(()), Ok(()));

        assert!(state.shutdown_with_timeout(std::time::Duration::from_secs(1)));
        assert!(started.elapsed() >= std::time::Duration::from_millis(20));
        assert!(completion.join().is_ok_and(|result| result.is_ok()));
    }

    #[test]
    fn command_acceptance_follows_the_channel_not_browser_status() {
        let mut state = BrowserPanelState::inert();
        let start = BrowserCommand::Video {
            operation: BrowserVideoOperation::Start,
            options: None,
        };
        assert!(!state.can_accept_commands());
        assert!(!state.try_send(start.clone()));

        state.status = BrowserStatus::Ready;
        assert!(!state.can_accept_commands());

        state.status = BrowserStatus::Starting;
        state.pending_relaunch = Some(PendingRelaunch { initial_url: None });
        assert!(!state.can_accept_commands());
        assert!(!state.try_send(start));
    }

    #[test]
    fn queued_retry_launches_after_teardown_without_another_click() {
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let mut state = BrowserPanelState {
            status: BrowserStatus::Starting,
            url: None,
            title: String::new(),
            loading: true,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: Some(Box::new(BrowserShutdownSignal::for_test(completion_rx))),
            pending_relaunch: Some(PendingRelaunch { initial_url: None }),
            panel_local_id: "queued-retry-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            // A launch that fails at once, from a program of this computer that
            // ends at once. A command that this computer does not have is read as
            // unset, and the launch would then use the browser that it has.
            config: BrowserConfig {
                command: Some(crate::util::stand_in_program().to_string()),
                ..BrowserConfig::default()
            },
            teach: None,
            shared_session: None,
        };

        assert!(!state.continue_pending_relaunch());
        assert_eq!(completion_tx.send(()), Ok(()));
        assert!(state.continue_pending_relaunch());
        assert!(state.pending_relaunch.is_none());
        assert!(state.session.is_some());
        // The program that ends at once closes its receiver asynchronously. A
        // retained session handle must not keep accepting commands after that exit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.can_accept_commands() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!state.can_accept_commands());
        assert!(state.session.is_some());
        assert!(!state.try_send(BrowserCommand::Reload));
        state.request_shutdown();
        if let Some(signal) = state.take_shutdown_signal() {
            assert!(signal.wait(std::time::Duration::from_secs(2)));
        }
    }

    #[test]
    fn profile_cleanup_waits_for_driver_teardown() {
        let root = tempfile::tempdir().expect("temp dir");
        let profile_dir = root.path().join("panel-profile");
        std::fs::create_dir(&profile_dir).expect("profile dir");
        std::fs::write(profile_dir.join("Preferences"), b"state").expect("profile state");
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();

        let signal = BrowserShutdownSignal::for_test(completion_rx).with_profile_cleanup(profile_dir.clone());
        assert!(profile_dir.exists());
        assert_eq!(completion_tx.send(()), Ok(()));
        assert!(signal.wait(std::time::Duration::from_secs(1)));
        assert!(!profile_dir.exists());
    }

    #[test]
    fn failed_profile_cleanup_is_not_acknowledged_as_complete() {
        let root = tempfile::tempdir().expect("temp dir");
        let profile_path = root.path().join("not-a-directory");
        std::fs::write(&profile_path, b"profile placeholder").expect("profile placeholder");
        let (completion_tx, completion_rx) = std::sync::mpsc::channel();
        let signal = BrowserShutdownSignal::for_test(completion_rx).with_profile_cleanup(profile_path.clone());
        assert_eq!(completion_tx.send(()), Ok(()));

        assert!(!signal.wait(std::time::Duration::from_secs(1)));
        assert!(!signal.is_complete());
        assert!(profile_path.exists());

        std::fs::remove_file(&profile_path).expect("remove profile placeholder");
        assert!(signal.force_cleanup(std::time::Duration::from_secs(1)));
        assert!(!profile_path.exists());
    }

    #[test]
    fn failed_hand_back_keeps_the_retry_affordance() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: None,
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "handoff-test".to_string(),
            requested_url: None,
            owner: Some("agent".to_string()),
            handoff_reason: Some("captcha".to_string()),
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        state.hand_back();

        assert_eq!(state.handoff_reason.as_deref(), Some("captcha"));
        assert!(state.handoff_error.is_some());
        assert!(!state.handoff_resolution_pending);
    }

    #[test]
    fn committed_url_is_applied_after_the_session_event_receiver_is_gone() {
        let committed_url = session::CommittedUrl::default();
        committed_url.publish("https://example.com/final");
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: Some("https://example.com/old".to_string()),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url,
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "final-url-test".to_string(),
            requested_url: None,
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: Some("stale error".to_string()),
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        let output = state.drain_events();

        assert_eq!(state.url.as_deref(), Some("https://example.com/final"));
        assert!(state.navigation_error.is_none());
        assert!(output.url_changed);
        assert!(output.had_output);
    }

    #[test]
    fn requested_url_stays_out_of_persistence_until_chrome_commits() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Starting,
            url: None,
            title: String::new(),
            loading: true,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "requested-url-test".to_string(),
            requested_url: Some("https://example.com/requested".to_string()),
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        assert_eq!(state.display_url(), "https://example.com/requested");
        assert_eq!(state.committed_url_for_persistence(), None);

        state.committed_url.publish("");
        let output = state.drain_events();

        assert_eq!(state.display_url(), "");
        assert_eq!(state.committed_url_for_persistence(), Some(""));
        assert!(output.url_changed);
    }

    #[test]
    fn relaunch_clears_agent_state_from_the_stopped_driver() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: None,
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            committed_url: session::CommittedUrl::default(),
            teardown_signal: None,
            pending_relaunch: None,
            panel_local_id: "relaunch-test".to_string(),
            requested_url: None,
            owner: Some("agent".to_string()),
            handoff_reason: Some("captcha".to_string()),
            handoff_error: Some("driver unavailable".to_string()),
            handoff_resolution_pending: true,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            video_error: None,
            remote_status: None,
            orientation: RemoteOrientationView::default(),
            orientation_pending_since: None,
            orientation_queue_rejection: None,
            remote: None,
            attached: None,
            host_driver: None,
            attach_endpoint: None,
            attached_page_target: None,
            attached_page_context: None,
            page_isolation: Default::default(),
            host_page_keeps_nothing: false,
            page_resumed: false,
            resumed_notice_at: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
            teach: None,
            shared_session: None,
        };

        state.clear_agent_state_for_relaunch();

        assert!(state.owner.is_none());
        assert!(state.handoff_reason.is_none());
        assert!(state.handoff_error.is_none());
        assert!(!state.handoff_resolution_pending);
    }
}
