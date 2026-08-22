//! Browser panels: headless Chrome driven over the Chrome DevTools
//! Protocol, rendered inside Horizon as first-class panels.
//!
//! Layout follows the module-boundary rules in
//! `docs/architecture/maintainability.md`:
//!
//! - `cdp` — DevTools Protocol transport (WebSocket JSON-RPC)
//! - `process` — Chrome binary discovery, spawn, kill
//! - `frames` — JPEG decode + shared frame slot
//! - `input` — core input model + CDP parameter mapping
//! - `session` — the per-panel driver thread and its state machine
//! - `manifest` — the file-based ownership/handoff channel shared with
//!   the `hb` agent CLI and the UI
//! - `snapshot` — canned JS page snapshotting for agents

pub mod cdp;
pub mod frames;
pub mod input;
pub mod manifest;
pub mod process;
pub mod session;
pub mod snapshot;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use std::sync::Arc;

pub use frames::FrameSlot;
pub use input::{
    BrowserButton, BrowserInput, BrowserKey, BrowserModifiers,
};
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
        .last()
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
    /// Screencast frame rate cap (frames are change-driven; this is a cap).
    pub max_fps: u32,
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
            max_fps: 15,
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
    Error { message: String },
    Stopped { code: Option<i32> },
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
    pub panel_local_id: String,
    /// Initial URL requested at (re)start, for the URL bar.
    pub requested_url: Option<String>,
    /// Agent currently driving this panel (from manifest heartbeats).
    pub owner: Option<String>,
    /// Pending handoff request from the agent, with its reason.
    pub handoff_reason: Option<String>,
    config: BrowserConfig,
}

impl BrowserPanelState {
    /// Create the state and start the driver thread.
    pub fn start(
        panel_local_id: impl Into<String>,
        config: &BrowserConfig,
        initial_url: Option<String>,
    ) -> Self {
        let panel_local_id = panel_local_id.into();
        let mut state = Self {
            status: BrowserStatus::Starting,
            url: initial_url
                .clone()
                .filter(|u| u != "about:blank")
                .unwrap_or_default(),
            title: String::new(),
            loading: true,
            frame_slot: Arc::new(FrameSlot::new()),
            session: None,
            requested_url: initial_url.clone(),
            panel_local_id: panel_local_id.clone(),
            owner: None,
            handoff_reason: None,
            config: config.clone(),
        };
        state.launch_session(initial_url);
        state
    }

    /// (Re)start the driver — used for the Retry action after a failure.
    pub fn relaunch(&mut self) {
        self.launch_session(self.requested_url.clone().filter(|u| !u.is_empty()));
    }

    fn launch_session(&mut self, initial_url: Option<String>) {
        // Drop the previous driver (if any); its Stop command makes it tear
        // down its Chrome process.
        if let Some(session) = self.session.take() {
            let _ = session.send(BrowserCommand::Stop);
        }
        let session_config = session::BrowserSessionConfig {
            browser: self.config.clone(),
            panel_local_id: self.panel_local_id.clone(),
            initial_url,
            width: DEFAULT_VIEWPORT.0,
            height: DEFAULT_VIEWPORT.1,
        };
        match session::start_session(session_config) {
            Ok(handle) => {
                self.status = BrowserStatus::Starting;
                self.frame_slot = Arc::clone(&handle.frame_slot);
                self.loading = true;
                self.session = Some(handle);
            }
            Err(message) => {
                self.status = BrowserStatus::Error { message };
            }
        }
    }

    pub fn send(&self, command: BrowserCommand) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    /// Tell the driver the user handed the panel back to the agent.
    pub fn hand_back(&mut self) {
        if let Some(session) = &self.session {
            let _ = session.send(BrowserCommand::HandoffDone);
        }
        self.handoff_reason = None;
    }

    pub fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.send(BrowserCommand::Stop);
        }
        if matches!(self.status, BrowserStatus::Starting | BrowserStatus::Ready) {
            self.status = BrowserStatus::Stopped { code: None };
        }
    }

    /// Drain driver events into panel state. Returns `true` when the panel
    /// should be considered to have produced visible output this frame
    /// (new frame, title change, or status change).
    pub fn drain_events(&mut self) -> bool {
        let events = self
            .session
            .as_ref()
            .map(|session| session.event_rx.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if events.is_empty() {
            return false;
        }
        let mut active = false;
        for event in events {
            match event {
                BrowserEvent::Ready => {
                    self.status = BrowserStatus::Ready;
                    active = true;
                }
                BrowserEvent::Title(title) => {
                    if self.title != title {
                        self.title = title;
                        active = true;
                    }
                }
                BrowserEvent::UrlChanged(url) => {
                    self.url = url;
                    active = true;
                }
                BrowserEvent::Loading(loading) => {
                    self.loading = loading;
                }
                BrowserEvent::Frame { .. } => {
                    active = true;
                }
                BrowserEvent::Warning(message) => {
                    self.status = BrowserStatus::Error { message };
                    active = true;
                }
                BrowserEvent::Stopped { code } => {
                    self.session = None;
                    self.status = BrowserStatus::Stopped { code };
                    active = true;
                }
                BrowserEvent::HandoffRequested(reason) => {
                    self.handoff_reason = Some(reason);
                    active = true;
                }
                BrowserEvent::HandoffCleared => {
                    if self.handoff_reason.is_some() {
                        self.handoff_reason = None;
                        active = true;
                    }
                }
                BrowserEvent::OwnerChanged(owner) => {
                    if self.owner != owner {
                        self.owner = owner;
                        active = true;
                    }
                }
            }
        }
        active
    }
}

impl Drop for BrowserPanelState {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Resolve the manifest directory for browser panels (UI + `hb` use this).
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
        assert_eq!(config.max_fps, 15);
        assert_eq!(config.quality, 60);
        assert!(config.command.is_none());
    }

    #[test]
    fn status_transitions() {
        assert!(BrowserStatus::default().is_alive());
        assert!(BrowserStatus::Ready.is_alive());
        assert!(!BrowserStatus::Stopped { code: None }.is_alive());
        assert!(!BrowserStatus::Error {
            message: "x".to_string()
        }
        .is_alive());
    }
}
