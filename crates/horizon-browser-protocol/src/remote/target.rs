use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::error::{ExtensionProblem, RemoteConfigError};

/// Hardware requirement Horizon verifies after allocation. This is a Horizon
/// rule, not a `WebDriver` capability; the adapter maps it to the provider's
/// real-device request and the lifecycle checks the provider's evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    #[default]
    Physical,
    Emulated,
    Any,
}

/// Requested device identity, compared against provider evidence by prefix.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceRequirement {
    pub kind: DeviceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

/// One selectable remote browser target. Agents pick targets by name and can
/// never supply endpoints, credentials or capability JSON themselves.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteTargetProfile {
    pub provider: String,
    pub browser_name: String,
    pub platform_name: String,
    #[serde(default)]
    pub device: DeviceRequirement,
    /// Namespaced provider capabilities (`vendor:name`). Standard capabilities,
    /// normalized target fields and credentials are rejected here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub capability_extensions: BTreeMap<String, serde_json::Value>,
}

/// Capability names (compared case-insensitively, after the namespace) that
/// the normalized fields already express.
const NORMALIZED_NAMES: &[&str] = &[
    "browsername",
    "browserversion",
    "platformname",
    "platformversion",
    "devicename",
    "osversion",
    "os_version",
    "realmobile",
    "device",
];

/// Exact capability names (lowercased, separators removed) that identify a
/// provider account, plus fragments that mark any key as secret-bearing.
const CREDENTIAL_NAMES: &[&str] = &["user", "username", "key", "auth", "credentials"];
const CREDENTIAL_FRAGMENTS: &[&str] = &[
    "secret",
    "token",
    "password",
    "passwd",
    "accesskey",
    "apikey",
    "authorization",
    "credential",
    "privatekey",
];

impl RemoteTargetProfile {
    pub(super) fn validate(&self, target: &str) -> Result<(), RemoteConfigError> {
        if !printable_ascii(&self.browser_name, 64) {
            return Err(RemoteConfigError::InvalidBrowserName {
                target: target.to_string(),
            });
        }
        if !printable_ascii(&self.platform_name, 64) {
            return Err(RemoteConfigError::InvalidPlatformName {
                target: target.to_string(),
            });
        }
        for (field, value) in [("model", &self.device.model), ("os_version", &self.device.os_version)] {
            if value.as_deref().is_some_and(|value| !printable_ascii(value, 128)) {
                return Err(RemoteConfigError::InvalidDeviceField {
                    target: target.to_string(),
                    field,
                });
            }
        }
        for (key, value) in &self.capability_extensions {
            if let Some(problem) = extension_problem(key, value) {
                return Err(RemoteConfigError::InvalidCapabilityExtension {
                    target: target.to_string(),
                    key: key.clone(),
                    problem,
                });
            }
        }
        Ok(())
    }
}

fn extension_problem(key: &str, value: &serde_json::Value) -> Option<ExtensionProblem> {
    let Some((namespace, name)) = key.split_once(':') else {
        return Some(ExtensionProblem::NotNamespaced);
    };
    if namespace.is_empty() || name.is_empty() || !printable_ascii(key, 128) {
        return Some(ExtensionProblem::NotNamespaced);
    }
    if let Some(problem) = name_problem(name) {
        return Some(problem);
    }
    if let serde_json::Value::Object(fields) = value {
        for nested in fields.keys() {
            if let Some(problem) = name_problem(nested) {
                return Some(problem);
            }
        }
    }
    None
}

fn name_problem(name: &str) -> Option<ExtensionProblem> {
    let lowered = name.to_ascii_lowercase();
    let folded: String = lowered.chars().filter(|c| !matches!(c, '_' | '-' | '.')).collect();
    if CREDENTIAL_NAMES.contains(&folded.as_str())
        || CREDENTIAL_FRAGMENTS.iter().any(|fragment| folded.contains(fragment))
    {
        return Some(ExtensionProblem::CarriesCredential);
    }
    if NORMALIZED_NAMES.contains(&lowered.as_str()) || NORMALIZED_NAMES.contains(&folded.as_str()) {
        return Some(ExtensionProblem::ConflictsWithNormalizedField);
    }
    None
}

fn printable_ascii(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        && value.trim() == value
}
