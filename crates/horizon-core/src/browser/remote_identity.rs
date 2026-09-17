//! Cached, human-readable session identity; requested settings never become evidence.

use horizon_browser::{DeviceEvidence, DeviceEvidenceSource, RemoteDeviceIdentity, RemoteSessionRequest};

/// Presentation text built when session evidence changes, not every frame.
#[derive(Debug)]
pub struct RemoteIdentityDisplay {
    provider: &'static str,
    requested: String,
    label: String,
    tooltip: String,
}

impl RemoteIdentityDisplay {
    pub(super) fn requested(request: &RemoteSessionRequest) -> Self {
        let provider = match request.evidence {
            DeviceEvidenceSource::BrowserstackSession { .. } => "BrowserStack",
            DeviceEvidenceSource::Capabilities => "Remote browser",
        };
        let capabilities = &request.capabilities;
        let mut parts = Vec::new();
        for field in ["browserName", "platformName"] {
            if let Some(text) = capabilities.get(field).and_then(serde_json::Value::as_str) {
                parts.push(friendly_name(text).to_string());
            }
        }
        parts.extend(request.device.model.iter().cloned());
        if let Some(version) = &request.device.os_version {
            parts.push(format!("OS {version}"));
        }
        let requested = parts.join(" · ");
        Self {
            provider,
            label: format!("{provider} · Awaiting confirmation"),
            tooltip: format!("Session identity has not been confirmed.\nRequested: {requested}"),
            requested,
        }
    }

    pub(super) fn ended() -> Self {
        Self {
            provider: "Remote browser",
            requested: String::new(),
            label: "Remote browser · Session ended".into(),
            tooltip: "Session ended. Create a new panel to obtain current browser and device details.".into(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.label = format!("{} · Session ended", self.provider);
        self.tooltip = "Session ended. Previous browser and device details have been cleared.".into();
    }

    pub(super) fn confirm(&mut self, identity: &RemoteDeviceIdentity) {
        let browser = identity.browser_name.as_deref().map(friendly_name);
        let os = identity.os_name.as_deref().map(friendly_name);
        let mut parts = vec![
            self.provider.to_string(),
            browser.unwrap_or("Browser unconfirmed").to_string(),
        ];
        parts.extend(identity.model.iter().map(ToString::to_string));
        parts.push(match (os, identity.os_version.as_deref()) {
            (Some(name), Some(version)) => format!("{name} {version}"),
            (Some(name), None) => format!("{name} (version unconfirmed)"),
            (None, Some(version)) => format!("OS unconfirmed ({version})"),
            (None, None) => "OS unconfirmed".to_string(),
        });
        self.label = parts.join(" · ");
        self.tooltip = format!(
            "{}\nBrowser version: {}\nHardware: {}\nSource: provider-reported session details.\nRequested: {}\nThe remote target fixes the browser.",
            self.label,
            identity.browser_version.as_deref().unwrap_or("unconfirmed"),
            identity.hardware.unwrap_or(DeviceEvidence::Unknown),
            self.requested,
        );
        if identity.model.is_none() {
            self.tooltip.push_str("\nDevice model: not reported by the provider.");
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn tooltip(&self) -> &str {
        &self.tooltip
    }
}

fn friendly_name(value: &str) -> &str {
    match value.to_ascii_lowercase().as_str() {
        "edge" | "msedge" | "microsoftedge" => "Edge",
        "chrome" | "google chrome" => "Chrome",
        "safari" | "iphone" | "ipad" => "Safari",
        "firefox" => "Firefox",
        "ios" => "iOS",
        "android" => "Android",
        "windows" => "Windows",
        "os x" | "mac" | "macos" => "macOS",
        _ => value,
    }
}

#[cfg(test)]
mod tests;
