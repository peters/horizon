//! Horizon board state for browser panels.
//!
//! Protocol, process, frame, input, and session ownership live in the
//! UI-independent `horizon-browser` crate. This module adapts that engine to
//! Horizon persistence, panels, and private MCP host coordination.

#[doc(hidden)]
pub mod manifest;

pub use horizon_browser::{cdp, frames, input, process, session};

use std::path::{Path, PathBuf};

use std::sync::Arc;

pub use horizon_browser::{
    ActiveBackendCapabilities, AutomationDisclosurePolicy, AutomationDisclosureStatus, BackendAvailability,
    BackendCapabilities, BackendKind, BrowserButton, BrowserCommand, BrowserConfig, BrowserEditCommand, BrowserEvent,
    BrowserEventWaker, BrowserInput, BrowserKey, BrowserModifiers, BrowserSession, BrowserShutdownSignal,
    DEFAULT_VIEWPORT, FrameDelivery, FrameMetrics, FrameSlot, PageScrollState, normalize_navigation_target,
};
const FORCED_CHROME_SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Short human-facing title for a URL (hostname, or a non-empty fallback).
#[must_use]
pub fn panel_title_for_url(url: &str) -> String {
    let input = url.trim();
    if input.is_empty() {
        return "Browser".to_string();
    }
    let Some((_, scheme_specific)) = input.split_once("://") else {
        return input.to_string();
    };
    let authority = scheme_specific.split(['/', '?', '#']).next().unwrap_or_default();
    let host_and_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_and_port.strip_prefix('[') {
        bracketed.split_once(']').map_or("", |(host, _)| host)
    } else {
        host_and_port.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        input.to_string()
    } else {
        host.to_string()
    }
}

/// Coarse liveness of the panel's browser session.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BrowserStatus {
    #[default]
    Starting,
    Ready,
    Error {
        message: String,
    },
    Stopped {
        code: Option<i32>,
    },
}

impl BrowserStatus {
    #[must_use]
    pub const fn is_alive(&self) -> bool {
        matches!(self, Self::Starting | Self::Ready)
    }
}

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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
        let panel_local_id = panel_local_id.into();
        let mut config = config.resolved_for_launch()?;
        let home = crate::horizon_home::HorizonHome::resolve();
        let profile_root_resolved = retain_effective_profile_root(&mut config, &home.root().join("browser-profiles"));
        let initial_url = initial_url.map(|url| normalize_navigation_target(&url));
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
            panel_local_id: panel_local_id.clone(),
            owner: None,
            handoff_reason: None,
            handoff_error: None,
            handoff_resolution_pending: false,
            pending_clipboard_text: None,
            host_focus_request: None,
            navigation_error: None,
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: profile_root_resolved,
            config,
        };
        state.launch_session(initial_url);
        Ok(state)
    }

    /// (Re)start the driver — used for the Retry action after a failure.
    /// Resumes at the last-viewed URL when one is known, so a crashed
    /// browser returns the user to where they were rather than the panel's
    /// initial URL.
    pub fn relaunch(&mut self) {
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
    pub const fn backend_capabilities(&self) -> BackendCapabilities {
        self.config.backend.capabilities()
    }

    #[must_use]
    pub fn active_backend_capabilities(&self) -> Option<ActiveBackendCapabilities> {
        self.frame_slot.active_backend_capabilities()
    }

    /// Stop the exact current driver and queue a replacement using another
    /// backend. The replacement starts only after the old process has been
    /// reaped, so panel switching cannot overlap profile or port ownership.
    pub fn switch_backend(&mut self, backend: BackendKind) {
        if self.config.backend == backend {
            return;
        }
        let target = self.relaunch_target();
        self.config.backend = backend;
        self.persisted_config_changed = true;
        self.frame_slot.clear();
        self.frame_slot.clear_backend_capabilities();
        self.launch_session(target);
    }

    fn launch_session(&mut self, initial_url: Option<String>) {
        self.navigation_error = None;
        self.frame_slot.clear_backend_capabilities();
        // Drop the previous driver (if any) and wait for its Chrome to be
        // gone: the replacement reuses the same profile directory, and a
        // second instance would lose the profile lock race.
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new(BrowserSession::shutdown_signal(*session)));
        }
        if !self.retry_ready() {
            self.pending_relaunch = Some(PendingRelaunch { initial_url });
            self.loading = true;
            self.status = BrowserStatus::Starting;
            return;
        }
        self.start_session(initial_url);
    }

    fn start_session(&mut self, initial_url: Option<String>) {
        let browser = self.config.clone();
        let home = crate::horizon_home::HorizonHome::resolve();
        let capture_directory = profile_dir_for_home(&browser, &home, &self.panel_local_id).join("captures");
        let session_config = session::BrowserSessionConfig {
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
            coordination: Some(Arc::new(manifest::ManifestCoordination::default())),
            capture_directory: Some(capture_directory),
        };
        match session::start_session(session_config) {
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
        self.start_session(pending.initial_url);
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
    pub fn needs_event_waker(&self) -> bool {
        self.session.as_ref().is_some_and(|session| session.needs_event_waker())
    }

    pub fn set_event_waker(&self, callback: BrowserEventWaker) {
        if let Some(session) = &self.session {
            session.set_event_waker(callback);
        }
    }

    /// Queue a command only when the driver session currently exists.
    #[must_use]
    pub fn try_send(&self, command: BrowserCommand) -> bool {
        // Chrome commands, and the input that can navigate from inside the
        // page (a click on a link, Enter in a form), count as user activity
        // that may take a pending startup navigation over, but only once the
        // driver accepted them; pointer moves and wheel events do not
        // navigate on their own.
        let may_navigate = matches!(
            command,
            BrowserCommand::Navigate(_)
                | BrowserCommand::Back
                | BrowserCommand::Forward
                | BrowserCommand::Reload
                | BrowserCommand::Input(
                    horizon_browser::BrowserInput::MousePress { .. } | horizon_browser::BrowserInput::KeyDown { .. }
                )
        );
        let accepted = self.session.as_ref().is_some_and(|session| session.send(command));
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
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.send(BrowserCommand::HandoffDone))
        {
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

    /// Permanently close this panel and remove its persistent Chrome profile
    /// only after the driver has released the profile lock.
    ///
    /// The returned signal is what actually starts the profile deletion: it
    /// must be polled or waited on (see [`BrowserShutdownSignal`]). Dropping
    /// it leaves the profile on disk permanently.
    #[must_use = "profile cleanup only starts when the returned signal is polled or waited on"]
    pub(crate) fn close_permanently(&mut self) -> BrowserShutdownSignal {
        self.request_shutdown();
        let profile_dir = profile_dir_for_home(
            &self.config,
            &crate::horizon_home::HorizonHome::resolve(),
            &self.panel_local_id,
        );
        if let Some(signal) = self.take_shutdown_signal() {
            signal.with_profile_cleanup(profile_dir)
        } else {
            BrowserShutdownSignal::completed_with_profile_cleanup(profile_dir)
        }
    }

    /// Whether a failed/stopped session has fully released its Chrome
    /// process and profile, making Retry safe. Polling is non-blocking so the
    /// recovery placeholder cannot freeze the UI.
    pub fn retry_ready(&mut self) -> bool {
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
        let relaunched = self.continue_pending_relaunch();
        let config_changed = std::mem::take(&mut self.persisted_config_changed);
        let events = self
            .session
            .as_ref()
            .map(|session| session.event_rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut output = BrowserDrainOutput {
            had_output: relaunched || config_changed,
            config_changed,
            ..BrowserDrainOutput::default()
        };
        for event in events {
            self.apply_event(event, &mut output);
        }
        self.sync_committed_url(&mut output);
        output
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
            BrowserEvent::Stopped { code } => self.apply_stopped(code, output),
            BrowserEvent::HandoffRequested(reason) => {
                self.handoff_reason = Some(reason);
                self.handoff_error = None;
                self.handoff_resolution_pending = false;
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
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(Box::new((*session).completion_signal()));
        }
        self.frame_slot.clear();
        let keep_error = matches!(self.status, BrowserStatus::Error { .. });
        if !keep_error || code.is_some() {
            self.status = BrowserStatus::Stopped { code };
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
    fn panel_titles_extract_dns_and_bracketed_ipv6_hosts() {
        assert_eq!(panel_title_for_url("https://example.com:8443/path"), "example.com");
        assert_eq!(panel_title_for_url("http://[::1]:3000/path"), "::1");
        assert_eq!(panel_title_for_url("https://user:secret@example.net/a"), "example.net");
    }

    #[test]
    fn panel_titles_keep_non_empty_fallbacks_for_hostless_urls() {
        assert_eq!(panel_title_for_url("file:///tmp/page.html"), "file:///tmp/page.html");
        assert_eq!(panel_title_for_url("about:blank"), "about:blank");
        assert_eq!(panel_title_for_url("  "), "Browser");
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig {
                command: Some("/definitely/missing/chrome".to_string()),
                ..BrowserConfig::default()
            },
        };

        assert!(!state.continue_pending_relaunch());
        assert_eq!(completion_tx.send(()), Ok(()));
        assert!(state.continue_pending_relaunch());
        assert!(state.pending_relaunch.is_none());
        assert!(state.session.is_some());
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
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
            pending_user_navigation: None,
            user_navigations: std::sync::atomic::AtomicU32::new(0),
            persisted_config_changed: false,
            config: BrowserConfig::default(),
        };

        state.clear_agent_state_for_relaunch();

        assert!(state.owner.is_none());
        assert!(state.handoff_reason.is_none());
        assert!(state.handoff_error.is_none());
        assert!(!state.handoff_resolution_pending);
    }
}
