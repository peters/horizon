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
pub use session::{BrowserCommand, BrowserEvent, BrowserSession};

use crate::horizon_home::HorizonHome;

/// Default emulated viewport for a freshly created browser panel.
pub const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

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
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub frame_slot: Arc<FrameSlot>,
    session: Option<BrowserSession>,
    /// Driver teardown that must finish before Retry or application exit can
    /// reuse/release this panel's Chrome profile.
    teardown_signal: Option<std::sync::mpsc::Receiver<()>>,
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
            url: initial_url.clone().filter(|u| u != "about:blank").unwrap_or_default(),
            title: String::new(),
            loading: true,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            teardown_signal: None,
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
        let resume = (!self.url.is_empty()).then(|| self.url.clone());
        let initial_url = resume.or_else(|| self.requested_url.clone().filter(|u| !u.is_empty()));
        self.launch_session(initial_url);
    }

    fn launch_session(&mut self, initial_url: Option<String>) {
        self.navigation_error = None;
        // Drop the previous driver (if any) and wait for its Chrome to be
        // gone: the replacement reuses the same profile directory, and a
        // second instance would lose the profile lock race.
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(BrowserSession::shutdown_signal(session));
        }
        if !self.retry_ready() {
            self.loading = false;
            self.status = BrowserStatus::Error {
                message: "the previous browser is still shutting down; retry in a moment".to_string(),
            };
            return;
        }
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
                self.clear_agent_state_for_relaunch();
                self.status = BrowserStatus::Starting;
                self.loading = true;
                self.session = Some(handle);
            }
            Err(message) => {
                self.status = BrowserStatus::Error { message };
            }
        }
    }

    fn clear_agent_state_for_relaunch(&mut self) {
        self.owner = None;
        self.handoff_reason = None;
        self.handoff_error = None;
        self.handoff_resolution_pending = false;
    }

    pub fn send(&self, command: BrowserCommand) {
        if let Some(session) = &self.session {
            let _ = session.send(command);
        }
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
        if let Some(session) = self.session.take() {
            self.teardown_signal = Some(session.shutdown_signal());
        }
        if matches!(self.status, BrowserStatus::Starting | BrowserStatus::Ready) {
            self.status = BrowserStatus::Stopped { code: None };
        }
    }

    /// Take the stored teardown-completion signal, if a shutdown was
    /// requested and not yet joined.
    pub fn take_shutdown_signal(&mut self) -> Option<std::sync::mpsc::Receiver<()>> {
        self.teardown_signal.take()
    }

    /// Whether a failed/stopped session has fully released its Chrome
    /// process and profile, making Retry safe. Polling is non-blocking so the
    /// recovery placeholder cannot freeze the UI.
    pub fn retry_ready(&mut self) -> bool {
        let Some(signal) = &self.teardown_signal else {
            return true;
        };
        match signal.try_recv() {
            Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.teardown_signal = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    /// Drain driver events into panel state, separating visible activity from
    /// URL changes that must dirty the persisted runtime state.
    pub fn drain_events(&mut self) -> BrowserDrainOutput {
        let events = self
            .session
            .as_ref()
            .map(|session| session.event_rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if events.is_empty() {
            return BrowserDrainOutput::default();
        }
        let mut output = BrowserDrainOutput::default();
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
                    if self.url != url {
                        self.url = url;
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
                    output.had_output = true;
                }
                BrowserEvent::Warning(message) => {
                    self.frame_slot.clear();
                    self.status = BrowserStatus::Error { message };
                    output.had_output = true;
                }
                BrowserEvent::Stopped { code } => {
                    if let Some(session) = self.session.take() {
                        self.teardown_signal = Some(session.completion_signal());
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
        output
    }
}

impl Drop for BrowserPanelState {
    fn drop(&mut self) {
        self.stop();
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
            url: String::new(),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            teardown_signal: Some(completion_rx),
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
    fn failed_hand_back_keeps_the_retry_affordance() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: String::new(),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            teardown_signal: None,
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
    fn relaunch_clears_agent_state_from_the_stopped_driver() {
        let mut state = BrowserPanelState {
            status: BrowserStatus::Stopped { code: None },
            url: String::new(),
            title: String::new(),
            loading: false,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            teardown_signal: None,
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
