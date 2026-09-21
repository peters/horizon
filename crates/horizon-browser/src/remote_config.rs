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
