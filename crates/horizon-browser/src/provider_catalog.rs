//! `BrowserStack`'s live combinations, never caller-supplied capabilities or endpoints.
use crate::{
    provider_usage::UsageAdapter,
    remote::{DeviceKind, DeviceRequirement, RemoteAdapterKind, RemoteProviderProfile, RemoteTargetProfile},
};
pub use horizon_browser_protocol::provider_catalog::{CatalogDevice, CatalogPage, CatalogQuery, target_provider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
mod cache;
pub use cache::CatalogCache;

const ENDPOINT: &str = "https://api.browserstack.com/automate/browsers.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CatalogError {
    #[error("provider_catalog_unsupported: device discovery is unavailable for this provider")]
    Unsupported,
    #[error("provider_catalog_credentials: provider credentials are unavailable")]
    Credentials,
    #[error("provider_catalog_access_denied: the provider refused this account")]
    AccessDenied,
    #[error("provider_catalog_unavailable: device discovery is temporarily unavailable")]
    Unavailable,
    #[error("provider_catalog_invalid: the provider returned an invalid device catalog")]
    InvalidResponse,
    #[error("provider_catalog_refresh_required: discover devices again before creating this target")]
    RefreshRequired,
}

/// # Errors
/// Only explicitly trusted provider hubs can delegate credentials to the fixed catalog API.
pub fn validate_provider(profile: &RemoteProviderProfile) -> Result<(), CatalogError> {
    if profile.adapter != RemoteAdapterKind::Browserstack
        || !UsageAdapter::Browserstack.authorizes_origin(&profile.endpoint.origin())
    {
        return Err(CatalogError::Unsupported);
    }
    Ok(())
}

/// Read-only discovery does not allocate or reserve capacity.
/// # Errors
/// Refused authentication, unavailable transport or malformed bounded data.
pub fn fetch(
    provider: &str,
    profile: &RemoteProviderProfile,
    authorization: &str,
) -> Result<Vec<CatalogDevice>, CatalogError> {
    validate_provider(profile)?;
    let config = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build();
    let mut response = ureq::Agent::new_with_config(config)
        .get(ENDPOINT)
        .header("Authorization", authorization)
        .header("Accept", "application/json")
        .call()
        .map_err(|_| CatalogError::Unavailable)?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(CatalogError::AccessDenied),
        _ => return Err(CatalogError::Unavailable),
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_vec()
        .map_err(|_| CatalogError::InvalidResponse)?;
    decode(provider, &bytes)
}

#[derive(Deserialize, Serialize)]
struct Row {
    os: String,
    os_version: String,
    browser: String,
    browser_version: Option<String>,
    device: Option<String>,
    real_mobile: Option<bool>,
}

/// # Errors
/// Rejects oversized or malformed catalogs without exposing provider response bodies.
pub fn decode(provider: &str, bytes: &[u8]) -> Result<Vec<CatalogDevice>, CatalogError> {
    let query = CatalogQuery {
        provider: provider.into(),
        ..Default::default()
    };
    if !query.valid() || bytes.len() > 8 * 1024 * 1024 {
        return Err(CatalogError::InvalidResponse);
    }
    let rows: Vec<Row> = serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidResponse)?;
    if rows.len() > 50_000 {
        return Err(CatalogError::InvalidResponse);
    }
    let mut devices = BTreeMap::new();
    for row in rows {
        for value in [
            Some(row.os.as_str()),
            Some(&row.os_version),
            Some(&row.browser),
            row.browser_version.as_deref(),
            row.device.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 128 || !value.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
                return Err(CatalogError::InvalidResponse);
            }
        }
        let canonical = serde_json::to_vec(&row).map_err(|_| CatalogError::InvalidResponse)?;
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(canonical) {
            use std::fmt::Write;
            let _ = write!(digest, "{byte:02x}");
        }
        let target = format!("catalog.{provider}.{digest}");
        devices.insert(
            target.clone(),
            CatalogDevice {
                target,
                provider: provider.into(),
                os: row.os,
                os_version: row.os_version,
                browser: row.browser,
                browser_version: row.browser_version,
                device: row.device,
                real_mobile: row.real_mobile.unwrap_or(false),
            },
        );
    }
    let mut devices: Vec<_> = devices.into_values().collect();
    devices.sort_by_cached_key(CatalogDevice::label);
    Ok(devices)
}

/// Convert only a discovered row into the common normalized target contract.
#[must_use]
pub fn target_profile(device: &CatalogDevice) -> RemoteTargetProfile {
    let browser = match device.browser.as_str() {
        "iphone" | "ipad" => "safari",
        "android" | "samsung" => "chrome",
        "ie" => "internet explorer",
        other => other,
    };
    RemoteTargetProfile {
        provider: device.provider.clone(),
        browser_name: browser.into(),
        platform_name: device.os.clone(),
        device: DeviceRequirement {
            kind: if device.real_mobile {
                DeviceKind::Physical
            } else if device.device.is_some() {
                DeviceKind::Emulated
            } else {
                DeviceKind::Any
            },
            model: device.device.clone(),
            os_version: Some(device.os_version.clone()),
        },
        capability_extensions: BTreeMap::new(),
    }
}

/// Versions from the provider catalog are normalized fields, never capability extensions from an agent.
pub fn apply_catalog_options(request: &mut crate::RemoteSessionRequest, device: &CatalogDevice) {
    if let Some(version) = &device.browser_version {
        request.capabilities["browserVersion"] = version.clone().into();
    }
    if device.device.is_none() {
        if let Some(object) = request.capabilities.as_object_mut() {
            object.remove("platformName");
        }
        request.capabilities["bstack:options"]["os"] = device.os.clone().into();
    }
    request.label.clone_from(&device.target);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_combinations_keep_stable_account_scoped_references_and_device_requirements() {
        let rows = br#"[{"os":"ios","os_version":"18","browser":"iphone","device":"iPhone 16","real_mobile":true,"browser_version":null},{"os":"Windows","os_version":"11","browser":"edge","device":null,"browser_version":"140.0","real_mobile":null}]"#;
        let devices = decode("account.one", rows).unwrap();
        assert_eq!(devices, decode("account.one", rows).unwrap());
        let mobile = devices.iter().find(|d| d.real_mobile).unwrap();
        assert_eq!(target_provider(&mobile.target), Some("account.one"));
        assert_eq!(target_profile(mobile).browser_name, "safari");
        assert_eq!(target_profile(mobile).device.kind, DeviceKind::Physical);
        assert_ne!(
            mobile.target,
            decode("account.two", rows)
                .unwrap()
                .iter()
                .find(|d| d.real_mobile)
                .unwrap()
                .target
        );
        let page = CatalogPage::select(
            &devices,
            &CatalogQuery {
                provider: "account.one".into(),
                search: "iphone 18".into(),
                offset: 0,
            },
        );
        assert_eq!(page.total, 1);
        assert_eq!(page.devices[0], *mobile);
        assert!(decode("account", br#"[{"os":"bad\n","os_version":"1","browser":"chrome"}]"#).is_err());
        assert!(target_provider("catalog.account.forged").is_none());
    }
}
