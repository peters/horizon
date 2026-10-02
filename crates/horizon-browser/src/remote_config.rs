//! Shared provider adaptation for UI, worker and command-line browser hosts.
use crate::remote::{
    BROWSERSTACK_OPTIONS_KEY, BROWSERSTACK_SESSION_API, DeviceKind, ExtensionProblem, RemoteAdapterKind,
    RemoteConfigError, RemoteProviderProfile, RemoteTargetProfile,
};
use crate::{BackendKind, DeviceEvidenceSource, RemoteAuthorizationHeader, RemoteSessionRequest};
use serde_json::{Map, Value};
use std::{sync::Arc, time::Duration};

/// Build a driver request from validated configuration and caller-resolved authentication.
/// # Errors
/// Rejects malformed provider capability extensions.
pub fn configured_remote_request(
    provider: &RemoteProviderProfile,
    target: &RemoteTargetProfile,
    target_name: &str,
    authorization: Option<Arc<RemoteAuthorizationHeader>>,
    quota_key: String,
) -> Result<RemoteSessionRequest, RemoteConfigError> {
    let limits = &provider.limits;
    Ok(RemoteSessionRequest {
        adapter: provider.adapter,
        recovery: crate::RemoteAllocation::default(),
        endpoint: provider.endpoint.as_str().to_string(),
        authorization,
        capabilities: capabilities_for(provider.adapter, target_name, target)?,
        allocation_timeout: Duration::from_secs(u64::from(limits.allocation_timeout_seconds)),
        max_session: Duration::from_secs(u64::from(limits.max_session_seconds)),
        idle_release: Duration::from_secs(u64::from(limits.idle_release_seconds)),
        label: target_name.to_string(),
        provider: target.provider.clone(),
        quota_key,
        browser: browser_family(&target.browser_name),
        device: target.device.clone(),
        evidence: evidence_source(provider.adapter),
    })
}

/// Apply a per-session orientation override through the same provider mapping.
pub fn override_orientation(request: &mut RemoteSessionRequest, orientation: Option<crate::remote::RemoteOrientation>) {
    let Some(orientation) = orientation else {
        return;
    };
    match request.adapter {
        RemoteAdapterKind::Browserstack => {
            request.capabilities[BROWSERSTACK_OPTIONS_KEY]["deviceOrientation"] = orientation.as_str().into();
        }
        RemoteAdapterKind::Webdriver => {
            request.capabilities["appium:orientation"] = orientation.webdriver_value().into();
        }
    }
}

/// Where the driver finds the allocated device's identity for this adapter:
/// the hosted grid's own session record, or the capabilities a standard
/// endpoint echoes.
fn evidence_source(adapter: RemoteAdapterKind) -> DeviceEvidenceSource {
    match adapter {
        RemoteAdapterKind::Webdriver => DeviceEvidenceSource::Capabilities,
        RemoteAdapterKind::Browserstack => DeviceEvidenceSource::BrowserstackSession {
            api_endpoint: BROWSERSTACK_SESSION_API.to_string(),
        },
    }
}

/// The local backend kind whose page semantics and capabilities match the
/// target's browser family. Anything that is not Safari or Firefox is
/// treated as Chromium.
#[must_use]
pub fn browser_family(browser_name: &str) -> BackendKind {
    let lowered = browser_name.to_ascii_lowercase();
    if lowered.contains("safari") {
        BackendKind::SafariWebDriver
    } else if lowered.contains("firefox") {
        BackendKind::FirefoxBidi
    } else {
        BackendKind::ChromiumCdp
    }
}

/// `alwaysMatch` capabilities: the standard browser and platform names, the
/// target's namespaced extensions verbatim, and the device fields placed
/// where the provider's adapter expects them. Configuration validation has
/// already refused extensions that carry those fields or credentials.
fn capabilities_for(
    adapter: RemoteAdapterKind,
    target_name: &str,
    target: &RemoteTargetProfile,
) -> Result<Value, RemoteConfigError> {
    let mut capabilities = Map::new();
    capabilities.insert("browserName".into(), Value::String(target.browser_name.clone()));
    capabilities.insert("platformName".into(), Value::String(target.platform_name.clone()));
    for (name, value) in &target.capability_extensions {
        capabilities.insert(name.clone(), value.clone());
    }
    let device = &target.device;
    match adapter {
        RemoteAdapterKind::Webdriver => {
            if let Some(orientation) = target.orientation {
                capabilities.insert(
                    "appium:orientation".into(),
                    Value::String(orientation.webdriver_value().into()),
                );
            }
            if let Some(model) = &device.model {
                capabilities.insert("appium:deviceName".into(), Value::String(model.clone()));
            }
            if let Some(version) = &device.os_version {
                capabilities.insert("appium:platformVersion".into(), Value::String(version.clone()));
            }
        }
        RemoteAdapterKind::Browserstack => {
            let options = capabilities
                .entry(BROWSERSTACK_OPTIONS_KEY)
                .or_insert_with(|| Value::Object(Map::new()));
            if !options.is_object() {
                // Configuration validation refuses this; never overwrite a
                // configured value silently.
                return Err(RemoteConfigError::InvalidCapabilityExtension {
                    target: target_name.to_string(),
                    key: BROWSERSTACK_OPTIONS_KEY.to_string(),
                    problem: ExtensionProblem::NotAnObject,
                });
            }
            if let Value::Object(options) = options {
                if let Some(orientation) = target.orientation {
                    options.insert("deviceOrientation".into(), Value::String(orientation.as_str().into()));
                }
                if let Some(model) = &device.model {
                    options.insert("deviceName".into(), Value::String(model.clone()));
                }
                if let Some(version) = &device.os_version {
                    options.insert("osVersion".into(), Value::String(version.clone()));
                }
                if device.kind == DeviceKind::Physical {
                    options.insert("realMobile".into(), Value::String("true".into()));
                }
            }
        }
    }
    Ok(Value::Object(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::RemoteOrientation;
    use serde_json::json;
    #[test]
    fn normalized_orientation_and_overrides_use_the_selected_adapter() {
        let mut target: RemoteTargetProfile =
            serde_json::from_value(json!({"provider":"grid","browser_name":"Safari","platform_name":"iOS"})).unwrap();
        for (adapter, key, path, expected) in [
            (RemoteAdapterKind::Webdriver, "appium:orientation", "", "LANDSCAPE"),
            (
                RemoteAdapterKind::Browserstack,
                "bstack:options",
                "deviceOrientation",
                "landscape",
            ),
        ] {
            let provider: RemoteProviderProfile =
                serde_json::from_value(json!({"adapter":adapter,"endpoint":"https://grid.example.test"})).unwrap();
            let mut request =
                configured_remote_request(&provider, &target, "tablet", None, "synthetic".into()).unwrap();
            assert_eq!(request.orientation(), None);
            override_orientation(&mut request, Some(RemoteOrientation::Portrait));
            assert_eq!(request.orientation(), Some(RemoteOrientation::Portrait));
            target.orientation = Some(RemoteOrientation::Landscape);
            let request = configured_remote_request(&provider, &target, "tablet", None, "synthetic".into()).unwrap();
            let value = if path.is_empty() {
                &request.capabilities[key]
            } else {
                &request.capabilities[key][path]
            };
            assert_eq!(value, expected);
            assert_eq!(request.orientation(), Some(RemoteOrientation::Landscape));
            target.orientation = None;
        }
    }

    #[test]
    fn resolved_startup_orientation_survives_later_target_and_request_changes() {
        for adapter in [RemoteAdapterKind::Webdriver, RemoteAdapterKind::Browserstack] {
            let provider: RemoteProviderProfile =
                serde_json::from_value(json!({"adapter":adapter,"endpoint":"https://grid.example.test"})).unwrap();
            for configured in [
                None,
                Some(RemoteOrientation::Portrait),
                Some(RemoteOrientation::Landscape),
            ] {
                for requested in [
                    None,
                    Some(RemoteOrientation::Portrait),
                    Some(RemoteOrientation::Landscape),
                ] {
                    let mut target: RemoteTargetProfile = serde_json::from_value(json!({
                        "provider":"grid","browser_name":"Safari","platform_name":"iOS","orientation":configured,
                    }))
                    .unwrap();
                    let mut request =
                        configured_remote_request(&provider, &target, "tablet", None, "synthetic".into()).unwrap();
                    override_orientation(&mut request, requested);
                    let startup_orientation = request.orientation();
                    assert_eq!(startup_orientation, requested.or(configured));
                    target.orientation = Some(RemoteOrientation::Landscape);
                    override_orientation(&mut request, Some(RemoteOrientation::Portrait));
                    assert_eq!(startup_orientation, requested.or(configured));
                    assert_eq!(request.orientation(), Some(RemoteOrientation::Portrait));
                    assert_eq!(target.orientation, Some(RemoteOrientation::Landscape));
                }
            }
        }
    }
}
