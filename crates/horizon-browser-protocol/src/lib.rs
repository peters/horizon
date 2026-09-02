#![forbid(unsafe_code)]

//! Lightweight serialized values for browser automation.
//!
//! This crate deliberately contains no browser process, transport, async
//! runtime, image decoding, filesystem coordination, MCP, or UI code.

mod audit;
mod command;
mod control;
pub mod input;
mod network;
mod semantic;

pub use audit::{
    BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id, redact_url,
};
pub use command::BrowserCommand;
pub use control::{
    AgentAction, BrowserControlAction, DEFAULT_CLICK_COUNT, DEFAULT_NAVIGATION_TIMEOUT_MILLIS,
    DEFAULT_WAIT_TIMEOUT_MILLIS, MAX_CLICK_COUNT, MAX_NAVIGATION_TIMEOUT_MILLIS, MAX_QUERY_RESULTS, MAX_SNAPSHOT_NODES,
    MAX_WAIT_TIMEOUT_MILLIS, NavigationWait, normalize_navigation_target,
};
pub use input::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};
pub use network::{
    BrowserNetworkCapture, BrowserNetworkCaptureOptions, BrowserNetworkConnection, BrowserNetworkConnectionState,
    BrowserNetworkDirection, BrowserNetworkEventKind, BrowserNetworkFrameOptions, BrowserNetworkOperation,
    BrowserNetworkPayloadEncoding, BrowserNetworkRecord, DEFAULT_NETWORK_MAX_FILE_BYTES,
    DEFAULT_NETWORK_MAX_PAYLOAD_BYTES, MAX_NETWORK_FILE_BYTES, MAX_NETWORK_PAYLOAD_BYTES, MAX_NETWORK_URL_PATTERNS,
};
pub use semantic::{
    AgentActionResult, BrowserActionOutcome, BrowserBounds, BrowserControlFailure, BrowserControlValue, BrowserNode,
    BrowserSnapshot, BrowserTarget, NavigationOutcome, NavigationState, SelectorState, WaitOutcome,
};

/// Browser automation backend selected for a session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum BackendKind {
    #[default]
    #[serde(rename = "chromium", alias = "chromium-cdp")]
    ChromiumCdp,
    #[serde(rename = "firefox", alias = "firefox-bidi")]
    FirefoxBidi,
    #[serde(rename = "safari", alias = "safari-web-driver")]
    SafariWebDriver,
}

/// How a backend can provide pixels to the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameDelivery {
    PushJpeg,
    AdaptiveScreenshot,
    RecordingFile,
    Unsupported,
}

/// Exact behavior callers may rely on for a selected backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // independent protocol feature claims
pub struct BackendCapabilities {
    pub frame_delivery: FrameDelivery,
    pub viewport: bool,
    pub physical_keys: bool,
    pub ime: bool,
    pub clipboard: bool,
    pub downloads: bool,
    pub persistent_profile: bool,
    /// Whether the backend can install common-signal minimization before
    /// author scripts run.
    pub automation_disclosure_minimization: bool,
    pub max_sessions: Option<u32>,
}

/// Compile-time host support for a backend. Executable discovery remains a
/// launch-time check because installations can change while Horizon runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendAvailability {
    Available,
    UnsupportedPlatform(&'static str),
}

impl BackendKind {
    #[must_use]
    pub const fn capabilities(self) -> BackendCapabilities {
        match self {
            Self::ChromiumCdp => BackendCapabilities {
                frame_delivery: FrameDelivery::PushJpeg,
                viewport: true,
                physical_keys: true,
                ime: true,
                clipboard: true,
                downloads: false,
                persistent_profile: true,
                automation_disclosure_minimization: true,
                max_sessions: None,
            },
            Self::FirefoxBidi => BackendCapabilities {
                frame_delivery: FrameDelivery::AdaptiveScreenshot,
                viewport: true,
                physical_keys: false,
                ime: true,
                clipboard: false,
                downloads: false,
                persistent_profile: true,
                automation_disclosure_minimization: true,
                max_sessions: None,
            },
            Self::SafariWebDriver => BackendCapabilities {
                frame_delivery: FrameDelivery::AdaptiveScreenshot,
                viewport: true,
                physical_keys: false,
                ime: false,
                clipboard: false,
                downloads: false,
                persistent_profile: false,
                automation_disclosure_minimization: false,
                max_sessions: Some(1),
            },
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ChromiumCdp => "Chromium",
            Self::FirefoxBidi => "Firefox",
            Self::SafariWebDriver => "Safari",
        }
    }

    #[must_use]
    pub const fn availability(self) -> BackendAvailability {
        match self {
            Self::SafariWebDriver if !cfg!(target_os = "macos") => {
                BackendAvailability::UnsupportedPlatform("Safari automation is available only on macOS")
            }
            _ => BackendAvailability::Available,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_and_aliases_round_trip() {
        for (backend, name) in [
            (BackendKind::ChromiumCdp, "chromium"),
            (BackendKind::FirefoxBidi, "firefox"),
            (BackendKind::SafariWebDriver, "safari"),
        ] {
            let encoded = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&backend).ok().as_deref(), Some(encoded.as_str()));
            assert_eq!(serde_json::from_str::<BackendKind>(&encoded).ok(), Some(backend));
        }
        assert_eq!(
            serde_json::from_str::<BackendKind>("\"firefox-bidi\"").ok(),
            Some(BackendKind::FirefoxBidi)
        );
    }
}
