#![forbid(unsafe_code)]

//! First-party browser automation and frame delivery for Horizon.
//!
//! The crate is UI-independent and keeps protocol, browser-process, input,
//! and frame ownership outside Horizon's board and persistence model.

mod audit;
pub mod cdp;
mod control;
mod coordination;
mod disclosure;
mod error;
pub mod frames;
pub mod input;
mod paths;
pub mod process;
mod profile;
pub mod session;
mod webdriver;
mod websocket;

use std::path::PathBuf;

pub use audit::{BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id};
pub use control::{AgentAction, BrowserControlAction};
pub use coordination::{BrowserCoordination, CoordinationSignals, CoordinationState, HandoffRequest};
pub use disclosure::{AutomationDisclosurePolicy, AutomationDisclosureStatus};
pub use error::BrowserError;
pub use frames::{FrameData, FrameMetrics, FrameSlot, PageScrollState};
pub use input::{BrowserButton, BrowserEditCommand, BrowserInput, BrowserKey, BrowserModifiers};
pub use session::{
    BrowserCommand, BrowserEvent, BrowserEventWaker, BrowserSession, BrowserSessionConfig, BrowserShutdownSignal,
    CommittedUrl, start_session,
};

/// Default emulated viewport for a newly created browser session.
pub const DEFAULT_VIEWPORT: (u32, u32) = (1280, 800);

/// Browser-engine launch configuration. Horizon owns serialization and
/// persistence policy; this value is the resolved launch contract.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserConfig {
    pub backend: BackendKind,
    /// Treatment of common script-visible browser-automation signals.
    pub automation_disclosure: AutomationDisclosurePolicy,
    /// Explicit Chromium executable (absolute path or PATH name).
    pub command: Option<String>,
    /// Explicit Firefox executable. Geckodriver still owns the process.
    pub firefox_command: Option<String>,
    /// Explicit geckodriver executable.
    pub geckodriver_command: Option<String>,
    /// Explicit safaridriver executable on macOS.
    pub safaridriver_command: Option<String>,
    /// Backend-specific extra process arguments.
    pub extra_args: Vec<String>,
    /// Chromium CDP screencast JPEG quality (1-100).
    pub quality: u32,
    /// Chromium CDP screencast sampling interval (`1` publishes every frame).
    pub every_nth_frame: u32,
    pub profile_root: Option<PathBuf>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            backend: BackendKind::ChromiumCdp,
            automation_disclosure: AutomationDisclosurePolicy::default(),
            command: None,
            firefox_command: None,
            geckodriver_command: None,
            safaridriver_command: None,
            extra_args: Vec::new(),
            quality: 60,
            every_nth_frame: 1,
            profile_root: None,
        }
    }
}

impl BrowserConfig {
    /// Resolve relative profile roots once on the calling thread and clamp
    /// protocol-bound values before the driver starts.
    ///
    /// # Errors
    /// Returns an error if the process launch directory cannot be resolved.
    pub fn resolved_for_launch(&self) -> std::io::Result<Self> {
        profile::resolved_for_launch(self)
    }

    #[must_use]
    pub fn profile_dir(&self, panel_local_id: &str) -> PathBuf {
        profile::profile_dir(self, panel_local_id)
    }
}

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

/// Runtime extensions negotiated for one active backend session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActiveBackendCapabilities {
    pub backend: BackendKind,
    pub capabilities: BackendCapabilities,
    pub bidi: bool,
    pub automation_disclosure: AutomationDisclosureStatus,
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
    fn backend_names_round_trip_through_user_facing_config_values() {
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

    #[test]
    fn backend_capabilities_preserve_protocol_differences() {
        assert_eq!(
            BackendKind::ChromiumCdp.capabilities().frame_delivery,
            FrameDelivery::PushJpeg
        );
        assert!(
            BackendKind::ChromiumCdp
                .capabilities()
                .automation_disclosure_minimization
        );
        assert_eq!(
            BackendKind::FirefoxBidi.capabilities().frame_delivery,
            FrameDelivery::AdaptiveScreenshot
        );
        assert!(
            BackendKind::FirefoxBidi
                .capabilities()
                .automation_disclosure_minimization
        );
        let safari = BackendKind::SafariWebDriver.capabilities();
        assert_eq!(safari.frame_delivery, FrameDelivery::AdaptiveScreenshot);
        assert_eq!(safari.max_sessions, Some(1));
        assert!(!safari.persistent_profile);
        assert!(!safari.automation_disclosure_minimization);
    }

    #[test]
    fn legacy_config_defaults_to_common_signal_minimization() {
        let config = serde_json::from_str::<BrowserConfig>(r#"{"backend":"firefox"}"#).unwrap_or_default();

        assert_eq!(config.backend, BackendKind::FirefoxBidi);
        assert_eq!(
            config.automation_disclosure,
            AutomationDisclosurePolicy::MinimizeCommonSignals
        );
        assert_eq!(
            serde_json::to_value(&config)
                .ok()
                .and_then(|value| value.get("automation_disclosure").cloned()),
            Some(serde_json::json!("minimize_common_signals"))
        );
    }

    #[test]
    fn safari_availability_matches_the_compile_target() {
        let availability = BackendKind::SafariWebDriver.availability();
        if cfg!(target_os = "macos") {
            assert_eq!(availability, BackendAvailability::Available);
        } else {
            assert!(matches!(availability, BackendAvailability::UnsupportedPlatform(_)));
        }
    }
}
