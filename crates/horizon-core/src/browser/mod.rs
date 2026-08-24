//! Browser panels: headless `Chrome` driven over the `CDP`
//! Protocol, rendered inside Horizon as first-class panels.
//!
//! Layout follows the module-boundary rules in
//! `docs/architecture/maintainability.md`:
//!
//! - `cdp` — `CDP` transport (websocket JSON-RPC)
//! - `process` — Chrome binary discovery, spawn, kill
//! - `frames` — JPEG decode + shared frame slot
//! - `input` — core input model + CDP parameter mapping
//! - `session` — the per-panel driver thread and its state machine
//! - `manifest` — the file-based ownership/handoff channel shared with
//!   external agents and the UI

pub mod cdp;
pub mod frames;
pub mod input;
pub mod manifest;
pub mod process;
pub mod session;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use std::sync::Arc;

pub use frames::FrameSlot;
pub use input::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};
pub(crate) use session::BrowserShutdownSignal;
pub use session::{BrowserCommand, BrowserEvent, BrowserEventWaker, BrowserSession};

use crate::horizon_home::HorizonHome;

/// Default emulated viewport for a freshly created browser panel.
pub const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);
const FORCED_CHROME_SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Short human-facing title for a URL (hostname, or the raw input).
#[must_use]
pub fn panel_title_for_url(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(url)
        .split('@')
        .next_back()
        .unwrap_or(url)
        .split(':')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// `browser` section of the Horizon config.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserConfig {
    /// Explicit Chrome/Chromium executable (absolute path or PATH name).
    /// When unset, a standard candidate list is scanned.
    pub command: Option<String>,
    /// Extra CLI arguments appended to every Chrome launch.
    pub extra_args: Vec<String>,
    /// JPEG quality 1-100 for screencast frames.
    pub quality: u32,
    /// Deliver every Nth screencast frame (1 = all frames, 2 = half rate).
    /// Chrome's only frame-rate knob; useful on animation-heavy pages to
    /// halve decode + texture-upload cost at a perceived 30 fps.
    pub every_nth_frame: u32,
    /// Override the per-panel profile root (default
    /// `~/.horizon/browser-profiles/<panel_id>`).
    pub profile_root: Option<PathBuf>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            command: None,
            extra_args: Vec::new(),
            quality: 60,
            every_nth_frame: 1,
            profile_root: None,
        }
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
    /// Most recent URL submission error; cleared after a committed navigation.
    pub navigation_error: Option<String>,
    config: BrowserConfig,
}

struct PendingRelaunch {
    initial_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserDrainOutput {
    pub had_output: bool,
    pub url_changed: bool,
}

impl BrowserPanelState {
    /// Create the state and start the driver thread.
    pub fn start(panel_local_id: impl Into<String>, config: &BrowserConfig, initial_url: Option<String>) -> Self {
        let panel_local_id = panel_local_id.into();
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
            navigation_error: None,
            config: config.clone(),
        };
        state.launch_session(initial_url);
        state
    }

    /// (Re)start the driver — used for the Retry action after a failure.
    /// Resumes at the last-viewed URL when one is known, so a crashed
    /// browser returns the user to where they were rather than the panel's
    /// initial URL.
    pub fn relaunch(&mut self) {
        let initial_url = match &self.url {
            Some(url) => (!url.is_empty()).then(|| url.clone()),
            None => self.requested_url.clone().filter(|url| !url.is_empty()),
        };
        self.launch_session(initial_url);
    }

    /// URL shown in browser chrome. Before Chrome commits anything, preserve
    /// the requested startup target as editable UI without treating it as
    /// persisted browser history.
    #[must_use]
    pub fn display_url(&self) -> &str {
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

    fn launch_session(&mut self, initial_url: Option<String>) {
        self.navigation_error = None;
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
        let session_config = session::BrowserSessionConfig {
            browser: self.config.clone(),
            panel_local_id: self.panel_local_id.clone(),
            initial_url,
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
            // Reuse the panel's slot across (re)starts: frame sequence
            // numbers stay monotonic, so a retried session's first frame is
            // never mistaken for an unchanged one by the UI's seq check.
            frame_slot: Arc::clone(&self.frame_slot),
        };
        match session::start_session(session_config) {
            Ok(handle) => {
                self.committed_url = handle.committed_url();
                self.clear_agent_state_for_relaunch();
                self.status = BrowserStatus::Starting;
                self.loading = true;
                self.session = Some(Box::new(handle));
            }
            Err(message) => {
                self.status = BrowserStatus::Error { message };
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
        self.session.as_ref().is_some_and(|session| session.send(command))
    }

    /// Take page text copied by headless Chrome for the UI's host clipboard.
    pub fn take_clipboard_text(&mut self) -> Option<String> {
        self.pending_clipboard_text.take()
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
    /// the driver finishes its Chrome teardown on its own thread.
    pub fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.send(BrowserCommand::Stop);
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
    pub(crate) fn close_permanently(&mut self) -> BrowserShutdownSignal {
        self.request_shutdown();
        let profile_dir = session::profile_dir(&self.config, &self.panel_local_id);
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
        let events = self
            .session
            .as_ref()
            .map(|session| session.event_rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut output = BrowserDrainOutput {
            had_output: relaunched,
            ..BrowserDrainOutput::default()
        };
        for event in events {
            match event {
                BrowserEvent::Ready => {
                    self.status = BrowserStatus::Ready;
                    output.had_output = true;
                }
                BrowserEvent::Title(title) => {
                    if self.title != title {
                        self.title = title;
                        output.had_output = true;
                    }
                }
                BrowserEvent::UrlChanged(url) => {
                    if self.url.as_deref() != Some(url.as_str()) {
                        self.url = Some(url);
                        output.url_changed = true;
                    }
                    self.navigation_error = None;
                    output.had_output = true;
                }
                BrowserEvent::NavigationFailed(message) => {
                    self.navigation_error = Some(message);
                    output.had_output = true;
                }
                BrowserEvent::Loading(loading) => {
                    self.loading = loading;
                }
                BrowserEvent::Frame { .. } => {
                    self.frame_slot.release_notification();
                    output.had_output = true;
                }
                BrowserEvent::Warning(message) => {
                    self.frame_slot.clear();
                    self.status = BrowserStatus::Error { message };
                    output.had_output = true;
                }
                BrowserEvent::Stopped { code } => {
                    if let Some(session) = self.session.take() {
                        self.teardown_signal = Some(Box::new((*session).completion_signal()));
                    }
                    // Drop the stale frame so the placeholder (with Retry)
                    // is shown instead of a frozen last frame.
                    self.frame_slot.clear();
                    // A startup/CDP failure arrives as Warning + Stopped;
                    // keep the actionable error text instead of showing a
                    // bare "stopped" state for it.
                    let keep_error = matches!(self.status, BrowserStatus::Error { .. });
                    if !keep_error || code.is_some() {
                        self.status = BrowserStatus::Stopped { code };
                    }
                    if self.handoff_resolution_pending {
                        self.handoff_resolution_pending = false;
                        self.handoff_error =
                            Some("browser stopped before the hand-back acknowledgement was saved".to_string());
                    }
                    output.had_output = true;
                }
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
                BrowserEvent::OwnerChanged(owner) => {
                    if self.owner != owner {
                        self.owner = owner;
                        output.had_output = true;
                    }
                }
            }
        }
        self.sync_committed_url(&mut output);
        output
    }

    fn sync_committed_url(&mut self, output: &mut BrowserDrainOutput) {
        let Some(url) = self.committed_url.snapshot() else {
            return;
        };
        if self.url.as_deref() == Some(url.as_str()) {
            return;
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

fn schedule_profile_removal(profile_dir: PathBuf) -> std::sync::mpsc::Receiver<()> {
    let fallback_profile_dir = profile_dir.clone();
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();
    let cleanup = move || {
        remove_browser_profile(&profile_dir);
        let _ = completion_tx.send(());
    };

    if let Err(error) = std::thread::Builder::new()
        .name("browser-profile-cleanup".into())
        .spawn(cleanup)
    {
        tracing::warn!("failed to start browser profile cleanup: {error}");
        remove_browser_profile(&fallback_profile_dir);
    }
    completion_rx
}

fn remove_browser_profile(profile_dir: &std::path::Path) {
    if let Err(error) = std::fs::remove_dir_all(profile_dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %profile_dir.display(), "failed to remove browser profile: {error}");
    }
}

/// Resolve the manifest directory for browser panels (UI + external agents).
#[must_use]
pub fn manifest_dir() -> PathBuf {
    HorizonHome::resolve().browsers_manifest_dir()
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
            navigation_error: None,
            config: BrowserConfig::default(),
        };

        assert!(!state.retry_ready());
        assert_eq!(completion_tx.send(()), Ok(()));
        assert!(state.retry_ready());
        assert!(state.teardown_signal.is_none());
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
            navigation_error: None,
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
            navigation_error: None,
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
            navigation_error: None,
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
            navigation_error: Some("stale error".to_string()),
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
            navigation_error: None,
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
            navigation_error: None,
            config: BrowserConfig::default(),
        };

        state.clear_agent_state_for_relaunch();

        assert!(state.owner.is_none());
        assert!(state.handoff_reason.is_none());
        assert!(state.handoff_error.is_none());
        assert!(!state.handoff_resolution_pending);
    }
}
