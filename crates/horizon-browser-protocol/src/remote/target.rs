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
const CREDENTIAL_NAMES: &[&str] = &["user", "username", "key", "credentials", "pin", "otp"];
const CREDENTIAL_FRAGMENTS: &[&str] = &[
    "secret",
    "token",
    "password",
    "passwd",
    "passphrase",
    "auth",
    "credential",
    "privatekey",
    "apikey",
    "accesskey",
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

/// `vendor:name` with exactly one colon and identifier characters on both
/// sides, so the credential matcher always sees the whole name.
fn extension_problem(key: &str, value: &serde_json::Value) -> Option<ExtensionProblem> {
    let mut parts = key.split(':');
    let (Some(namespace), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Some(ExtensionProblem::NotNamespaced);
    };
    if !identifier(namespace, 64) || !identifier(name, 64) {
        return Some(ExtensionProblem::NotNamespaced);
    }
    if let Some(problem) = name_problem(name) {
        return Some(problem);
    }
    nested_problem(value)
}

/// Every key at every depth of an extension value is checked, so a
/// credential or a normalized field cannot hide one object or array deeper
/// than the top level of the options.
fn nested_problem(value: &serde_json::Value) -> Option<ExtensionProblem> {
    match value {
        serde_json::Value::Object(fields) => {
            for (nested, inner) in fields {
                if !identifier(nested, 128) {
                    return Some(ExtensionProblem::InvalidOptionKey);
                }
                if let Some(problem) = name_problem(nested) {
                    return Some(problem);
                }
                if let Some(problem) = nested_problem(inner) {
                    return Some(problem);
                }
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().find_map(nested_problem),
        _ => None,
    }
}

fn identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn name_problem(name: &str) -> Option<ExtensionProblem> {
    let lowered = name.to_ascii_lowercase();
    let folded: String = lowered.chars().filter(|c| !matches!(c, '_' | '-' | '.')).collect();
    if CREDENTIAL_NAMES.contains(&folded.as_str())
        || folded.ends_with("key")
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
