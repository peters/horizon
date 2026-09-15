//! Requested versus actual device identity. A target states what it needs
//! (`physical`, a model, an OS version); after allocation the driver gathers
//! what the provider actually handed out and refuses a session that does
//! not satisfy the requirement, releasing it at once. Evidence never comes
//! from a user-agent string or a viewport size.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use super::super::remote_http::RemoteHttpClient;
use super::super::transport::ClassicTransport;
use horizon_browser_protocol::remote::{DeviceKind, DeviceRequirement};

/// Bounded wait for the provider's session record.
const RECORD_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest identity field accepted from a provider, the same bound the
/// configured model and OS version fields have.
const MAX_EVIDENCE_LEN: usize = 128;

/// A provider-reported identity field, or `None` when it is empty, longer
/// than a configured field may be, or carries control characters. Invalid
/// evidence is absent evidence: it is never copied into manifests or
/// results.
fn evidence_text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    if text.is_empty() || text.len() > MAX_EVIDENCE_LEN || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_string())
}

/// Where the driver looks for the allocated device's identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceEvidenceSource {
    /// The New Session reply's returned capabilities (Appium echoes
    /// `appium:deviceName` and `appium:platformVersion` on a self-hosted
    /// endpoint). Hardware is unknown unless a capability says real mobile.
    Capabilities,
    /// The hosted grid's own session record, fetched with the same
    /// authorization from `api_endpoint` (an HTTPS origin).
    BrowserstackSession { api_endpoint: String },
}

/// What the provider's evidence says about the hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceEvidence {
    Physical,
    Emulated,
    Unknown,
}

impl fmt::Display for DeviceEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Physical => "physical device",
            Self::Emulated => "emulated device",
            Self::Unknown => "unverified hardware",
        })
    }
}

/// The allocated device as the provider's evidence describes it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoteDeviceIdentity {
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub hardware: Option<DeviceEvidence>,
}

impl RemoteDeviceIdentity {
    /// One line for panels, manifests and audit: model, OS version and the
    /// hardware evidence, each only when known.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(model) = &self.model {
            parts.push(model.clone());
        }
        if let Some(version) = &self.os_version {
            parts.push(format!("OS {version}"));
        }
        parts.push(self.hardware.unwrap_or(DeviceEvidence::Unknown).to_string());
        parts.join(", ")
    }
}

/// Compare the requirement with the evidence. Models compare exactly
/// (case-insensitively); versions compare by leading components, so a
/// requested `18` accepts a resolved `18.6`; the hardware kind must match
/// when the requirement is not `any`. Anything the evidence does not say
/// fails a requirement that needs it.
///
/// # Errors
/// The unmet requirement, naming only configured and provider-reported
/// identity fields.
pub fn check_requirement(requirement: &DeviceRequirement, identity: &RemoteDeviceIdentity) -> Result<(), String> {
    if let Some(wanted) = &requirement.model {
        match &identity.model {
            Some(actual) if actual.eq_ignore_ascii_case(wanted) => {}
            Some(actual) => return Err(format!("device model is {actual}, target requires {wanted}")),
            None => return Err(format!("device model is unverified, target requires {wanted}")),
        }
    }
    if let Some(wanted) = &requirement.os_version {
        match &identity.os_version {
            Some(actual) if version_matches(wanted, actual) => {}
            Some(actual) => return Err(format!("OS version is {actual}, target requires {wanted}")),
            None => return Err(format!("OS version is unverified, target requires {wanted}")),
        }
    }
    let wanted = match requirement.kind {
        DeviceKind::Any => return Ok(()),
        DeviceKind::Physical => DeviceEvidence::Physical,
        DeviceKind::Emulated => DeviceEvidence::Emulated,
    };
    let actual = identity.hardware.unwrap_or(DeviceEvidence::Unknown);
    if actual == wanted {
        Ok(())
    } else {
        let article = match wanted {
            DeviceEvidence::Emulated => "an",
            DeviceEvidence::Physical | DeviceEvidence::Unknown => "a",
        };
        Err(format!(
            "target requires {article} {wanted}, provider evidence: {actual}"
        ))
    }
}

/// `requested` matches `actual` when every requested component leads the
/// actual version: `18` accepts `18.6`, `16.0` accepts `16.0`, `18.5` does
/// not accept `18.6`.
#[must_use]
pub fn version_matches(requested: &str, actual: &str) -> bool {
    let requested: Vec<&str> = requested.trim().split('.').collect();
    let actual: Vec<&str> = actual.trim().split('.').collect();
    actual.len() >= requested.len() && actual[..requested.len()] == requested[..]
}

/// Identity from the New Session reply: Appium namespaced fields first,
/// bare names as a fallback. Hardware is physical only when the reply says
/// so; nothing here is inferred from a user agent.
#[must_use]
pub fn identity_from_capabilities(capabilities: &Value) -> RemoteDeviceIdentity {
    let string = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| capabilities.get(*name).and_then(evidence_text))
    };
    let real_mobile = ["appium:realMobile", "realMobile", "appium:isRealMobile"]
        .iter()
        .find_map(|name| capabilities.get(*name))
        .and_then(|value| match value {
            Value::Bool(flag) => Some(*flag),
            Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        });
    RemoteDeviceIdentity {
        model: string(&["appium:deviceName", "deviceName"]),
        os_version: string(&["appium:platformVersion", "platformVersion"]),
        hardware: real_mobile.map(|real| {
            if real {
                DeviceEvidence::Physical
            } else {
                DeviceEvidence::Emulated
            }
        }),
    }
}

/// Identity from a hosted grid's session record (`automation_session`):
/// the record names the real device it ran on, or no device at all for a
/// desktop or emulator-less session.
#[must_use]
pub fn identity_from_session_record(record: &Value) -> RemoteDeviceIdentity {
    let session = record.get("automation_session").unwrap_or(record);
    let text = |name: &str| session.get(name).and_then(evidence_text);
    let model = text("device");
    RemoteDeviceIdentity {
        hardware: Some(if model.is_some() {
            DeviceEvidence::Physical
        } else {
            DeviceEvidence::Unknown
        }),
        os_version: text("os_version"),
        model,
    }
}

/// Fetch the grid's record for `session_id` with the hub's authorization.
///
/// # Errors
/// A transport, decoding or `WebDriver`-level failure as text; the caller
/// decides what an absent record means for the requirement.
pub(super) fn fetch_session_record(
    api_endpoint: &str,
    authorization: Option<super::super::remote_http::RemoteAuthorizationHeader>,
    session_id: &str,
) -> Result<Value, String> {
    let client = RemoteHttpClient::new(api_endpoint, authorization).map_err(|error| error.to_string())?;
    let segment = super::super::transport::encode_path_segment(session_id);
    if session_id.contains(['/', '%', '\\']) || segment.is_empty() {
        return Err("session id cannot form a record path".to_string());
    }
    client
        .get_with_read_timeout(&format!("/automate/sessions/{segment}.json"), RECORD_TIMEOUT)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        DeviceEvidence, RemoteDeviceIdentity, check_requirement, identity_from_capabilities,
        identity_from_session_record, version_matches,
    };
    use horizon_browser_protocol::remote::{DeviceKind, DeviceRequirement};

    fn requirement(kind: DeviceKind, model: Option<&str>, version: Option<&str>) -> DeviceRequirement {
        DeviceRequirement {
            kind,
            model: model.map(str::to_string),
            os_version: version.map(str::to_string),
        }
    }

    #[test]
    fn versions_match_by_leading_components() {
        assert!(version_matches("18", "18.6"));
        assert!(version_matches("16.0", "16.0"));
        assert!(version_matches("18.6", "18.6.1"));
        assert!(!version_matches("18.5", "18.6"));
        assert!(!version_matches("18.6", "18"));
        assert!(!version_matches("1", "18"));
    }

    #[test]
    fn a_physical_requirement_needs_physical_evidence_and_a_matching_identity() {
        let physical = RemoteDeviceIdentity {
            model: Some("iPhone 16".into()),
            os_version: Some("18.6".into()),
            hardware: Some(DeviceEvidence::Physical),
        };
        assert_eq!(
            check_requirement(
                &requirement(DeviceKind::Physical, Some("iphone 16"), Some("18")),
                &physical
            ),
            Ok(())
        );
        let error = check_requirement(&requirement(DeviceKind::Physical, Some("iPhone 15"), None), &physical)
            .expect_err("model differs");
        assert_eq!(error, "device model is iPhone 16, target requires iPhone 15");
        let error = check_requirement(&requirement(DeviceKind::Physical, None, Some("17")), &physical)
            .expect_err("version differs");
        assert_eq!(error, "OS version is 18.6, target requires 17");

        let unknown = RemoteDeviceIdentity {
            model: None,
            os_version: None,
            hardware: None,
        };
        let error =
            check_requirement(&requirement(DeviceKind::Physical, None, None), &unknown).expect_err("unverified");
        assert_eq!(
            error,
            "target requires a physical device, provider evidence: unverified hardware"
        );
        let error = check_requirement(&requirement(DeviceKind::Physical, Some("iPhone 16"), None), &unknown)
            .expect_err("unverified model");
        assert_eq!(error, "device model is unverified, target requires iPhone 16");
        assert_eq!(
            check_requirement(&requirement(DeviceKind::Any, None, None), &unknown),
            Ok(())
        );

        let emulated = RemoteDeviceIdentity {
            hardware: Some(DeviceEvidence::Emulated),
            ..RemoteDeviceIdentity::default()
        };
        assert_eq!(
            check_requirement(&requirement(DeviceKind::Physical, None, None), &emulated),
            Err("target requires a physical device, provider evidence: emulated device".into())
        );
        assert_eq!(
            check_requirement(&requirement(DeviceKind::Emulated, None, None), &emulated),
            Ok(())
        );
        assert_eq!(
            check_requirement(&requirement(DeviceKind::Emulated, None, None), &physical),
            Err("target requires an emulated device, provider evidence: physical device".into())
        );
    }

    #[test]
    fn capabilities_yield_identity_only_from_explicit_fields() {
        let identity = identity_from_capabilities(&json!({
            "browserName": "chrome",
            "appium:deviceName": "Pixel 9",
            "appium:platformVersion": "16.0",
            "appium:realMobile": "true"
        }));
        assert_eq!(
            identity,
            RemoteDeviceIdentity {
                model: Some("Pixel 9".into()),
                os_version: Some("16.0".into()),
                hardware: Some(DeviceEvidence::Physical),
            }
        );
        let bare = identity_from_capabilities(&json!({"deviceName": " emulator-5554 ", "realMobile": false}));
        assert_eq!(bare.model.as_deref(), Some("emulator-5554"));
        assert_eq!(bare.hardware, Some(DeviceEvidence::Emulated));
        let silent = identity_from_capabilities(&json!({"browserName": "safari", "platformName": "iOS"}));
        assert_eq!(silent, RemoteDeviceIdentity::default());
        assert_eq!(silent.summary(), "unverified hardware");

        // Oversized or control-laden provider text is absent evidence.
        let huge = "x".repeat(129);
        let bad = identity_from_capabilities(&json!({
            "appium:deviceName": huge,
            "appium:platformVersion": "18.6\u{7}",
        }));
        assert_eq!(bad, RemoteDeviceIdentity::default());
        let bad_record =
            identity_from_session_record(&json!({"automation_session": {"device": "a\nb", "os_version": "16.0"}}));
        assert_eq!(bad_record.model, None);
        assert_eq!(
            bad_record.hardware,
            Some(DeviceEvidence::Unknown),
            "a device name that is not evidence proves nothing"
        );
    }

    #[test]
    fn a_session_record_names_the_real_device_it_ran_on() {
        let identity = identity_from_session_record(&json!({
            "automation_session": {"device": "iPhone 16", "os": "ios", "os_version": "18.6", "browser": "iphone"}
        }));
        assert_eq!(identity.model.as_deref(), Some("iPhone 16"));
        assert_eq!(identity.os_version.as_deref(), Some("18.6"));
        assert_eq!(identity.hardware, Some(DeviceEvidence::Physical));
        assert_eq!(identity.summary(), "iPhone 16, OS 18.6, physical device");

        let desktop = identity_from_session_record(&json!({
            "automation_session": {"device": null, "os": "OS X", "os_version": "Sonoma"}
        }));
        assert_eq!(desktop.model, None);
        assert_eq!(desktop.hardware, Some(DeviceEvidence::Unknown));
        assert!(check_requirement(&requirement(DeviceKind::Physical, None, None), &desktop).is_err());
    }
}
