//! Turn a configured remote target into the request the driver runs. The
//! agent names a target; everything else (endpoint, capabilities, limits,
//! the authorization header) comes from configuration and the credential
//! stores on this computer, so nothing provider-specific crosses the MCP
//! boundary and no credential value is ever handed to a caller.

use std::sync::Arc;
use std::time::Duration;

use horizon_browser::remote::{
    BROWSERSTACK_OPTIONS_KEY, DeviceKind, ExtensionProblem, RemoteAdapterKind, RemoteBrowserConfig, RemoteConfigError,
    RemoteTargetProfile,
};
use horizon_browser::{BackendKind, RemoteAuthorizationHeader, RemoteSessionRequest};
use serde_json::{Map, Value};

use crate::remote_browser_credential::{CredentialStores, ResolveError, resolve_authorization};

/// Why a target could not become a session request. Messages name
/// identifiers only.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteRequestError {
    #[error("remote target `{target}` is not configured")]
    UnknownTarget { target: String },
    #[error("remote target `{target}` names provider `{provider}`, which is not configured")]
    UnknownProvider { target: String, provider: String },
    #[error("remote configuration is invalid: {0}")]
    Definition(RemoteConfigError),
    #[error("provider `{provider}` credentials are not ready: {error}")]
    Credential { provider: String, error: ResolveError },
    #[error("provider `{provider}` authorization could not be formed as a header")]
    Header { provider: String },
}

/// Build the driver request for `target_name`, resolving the provider's
/// authorization from `stores` now so allocation never touches a store.
///
/// # Errors
/// See [`RemoteRequestError`]; a missing, locked or malformed credential is
/// reported by reference name, never by value.
pub fn build_remote_session_request(
    remote: &RemoteBrowserConfig,
    target_name: &str,
    stores: &CredentialStores<'_>,
) -> Result<RemoteSessionRequest, RemoteRequestError> {
    let target = remote
        .targets
        .get(target_name)
        .ok_or_else(|| RemoteRequestError::UnknownTarget {
            target: target_name.to_string(),
        })?;
    let provider = remote
        .providers
        .get(&target.provider)
        .ok_or_else(|| RemoteRequestError::UnknownProvider {
            target: target_name.to_string(),
            provider: target.provider.clone(),
        })?;
    remote.validate_definition().map_err(RemoteRequestError::Definition)?;
    let authorization = resolve_authorization(provider, stores)
        .map_err(|error| RemoteRequestError::Credential {
            provider: target.provider.clone(),
            error,
        })?
        .map(|authorization| RemoteAuthorizationHeader::new(authorization.header_value().to_string()))
        .transpose()
        .map_err(|_| RemoteRequestError::Header {
            provider: target.provider.clone(),
        })?
        .map(Arc::new);
    let limits = &provider.limits;
    Ok(RemoteSessionRequest {
        endpoint: provider.endpoint.as_str().to_string(),
        authorization,
        capabilities: capabilities_for(provider.adapter, target_name, target)?,
        allocation_timeout: Duration::from_secs(u64::from(limits.allocation_timeout_seconds)),
        max_session: Duration::from_secs(u64::from(limits.max_session_seconds)),
        idle_release: Duration::from_secs(u64::from(limits.idle_release_seconds)),
        label: target_name.to_string(),
        browser: browser_family(&target.browser_name),
    })
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
) -> Result<Value, RemoteRequestError> {
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
                return Err(RemoteRequestError::Definition(
                    RemoteConfigError::InvalidCapabilityExtension {
                        target: target_name.to_string(),
                        key: BROWSERSTACK_OPTIONS_KEY.to_string(),
                        problem: ExtensionProblem::NotAnObject,
                    },
                ));
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use horizon_browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
    };
    use serde_json::json;

    use super::*;
    use crate::remote_browser_credential::{
        CredentialLocator, FakeCredentialStore, RemoteCredentialError, RemoteCredentialStore, SessionCredentialStore,
    };

    fn config() -> RemoteBrowserConfig {
        let mut bindings = BTreeMap::new();
        bindings.insert(
            CredentialReference::from("user"),
            CredentialBinding {
                store: CredentialStoreKind::Session,
                slot: None,
            },
        );
        bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::OsKeychain,
                slot: Some("remote-browser/grid/key".into()),
            },
        );
        let mut providers = BTreeMap::new();
        providers.insert(
            "grid".to_string(),
            RemoteProviderProfile {
                adapter: RemoteAdapterKind::Webdriver,
                endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint"),
                authentication: RemoteAuthentication::Basic {
                    username_ref: CredentialReference::from("user"),
                    password_ref: CredentialReference::from("key"),
                },
                credential_bindings: bindings,
                limits: RemoteSessionLimits {
                    max_sessions: 1,
                    allocation_timeout_seconds: 120,
                    idle_release_seconds: 180,
                    max_session_seconds: 1800,
                },
            },
        );
        let mut extensions = BTreeMap::new();
        extensions.insert("appium:automationName".to_string(), json!("XCUITest"));
        extensions.insert("bstack:options".to_string(), json!({"projectName": "horizon"}));
        let mut targets = BTreeMap::new();
        targets.insert(
            "ios_phone".to_string(),
            RemoteTargetProfile {
                provider: "grid".into(),
                browser_name: "safari".into(),
                platform_name: "ios".into(),
                device: horizon_browser::remote::DeviceRequirement {
                    kind: DeviceKind::Physical,
                    model: Some("iPhone 16".into()),
                    os_version: Some("18".into()),
                },
                capability_extensions: extensions,
            },
        );
        RemoteBrowserConfig { providers, targets }
    }

    fn locator(config: &RemoteBrowserConfig, reference: &str) -> CredentialLocator {
        let provider = &config.providers["grid"];
        let reference = CredentialReference::from(reference);
        CredentialLocator::new(
            &provider.endpoint,
            &reference,
            &provider.credential_bindings[&reference],
        )
    }

    #[test]
    fn a_target_becomes_a_request_with_the_resolved_header_and_configured_limits() {
        let config = config();
        let mut session = SessionCredentialStore::new();
        session.put(&locator(&config, "user"), b"alice").expect("user");
        let mut keychain = FakeCredentialStore::new();
        keychain.put(&locator(&config, "key"), b"s3cret").expect("key");
        let stores = CredentialStores {
            session: &session,
            os_keychain: Some(&keychain),
        };
        let request = build_remote_session_request(&config, "ios_phone", &stores).expect("request");
        assert_eq!(request.endpoint, "https://grid.example.net/wd/hub");
        assert_eq!(request.label, "ios_phone");
        assert_eq!(
            request.browser,
            BackendKind::SafariWebDriver,
            "the target's browser family travels with the request"
        );
        assert_eq!(browser_family("Chrome"), BackendKind::ChromiumCdp);
        assert_eq!(browser_family("firefox"), BackendKind::FirefoxBidi);
        assert!(request.authorization.is_some());
        assert_eq!(request.allocation_timeout, Duration::from_mins(2));
        assert_eq!(request.idle_release, Duration::from_mins(3));
        assert_eq!(request.max_session, Duration::from_mins(30));
        assert_eq!(request.capabilities["browserName"], "safari");
        assert_eq!(request.capabilities["platformName"], "ios");
        assert_eq!(request.capabilities["appium:automationName"], "XCUITest");
        assert_eq!(
            request.capabilities["appium:deviceName"], "iPhone 16",
            "the generic adapter uses Appium device capabilities"
        );
        assert_eq!(request.capabilities["appium:platformVersion"], "18");
        assert_eq!(
            request.capabilities["bstack:options"],
            json!({"projectName": "horizon"}),
            "extensions pass through untouched"
        );
        let debug = format!("{request:?}");
        assert!(!debug.contains("s3cret") && !debug.contains("alice"), "{debug}");
    }

    #[test]
    fn the_browserstack_adapter_places_the_device_request_in_its_options_object() {
        let mut config = config();
        config.providers.get_mut("grid").expect("provider").adapter = RemoteAdapterKind::Browserstack;
        let mut session = SessionCredentialStore::new();
        session.put(&locator(&config, "user"), b"alice").expect("user");
        let mut keychain = FakeCredentialStore::new();
        keychain.put(&locator(&config, "key"), b"s3cret").expect("key");
        let stores = CredentialStores {
            session: &session,
            os_keychain: Some(&keychain),
        };
        let request = build_remote_session_request(&config, "ios_phone", &stores).expect("request");
        assert_eq!(
            request.capabilities["bstack:options"],
            json!({"projectName": "horizon", "deviceName": "iPhone 16", "osVersion": "18", "realMobile": "true"}),
            "device fields merge into the existing options object"
        );
        assert!(request.capabilities.get("appium:deviceName").is_none());

        let mut scalar = config.clone();
        scalar
            .targets
            .get_mut("ios_phone")
            .expect("target")
            .capability_extensions
            .insert("bstack:options".into(), json!("not an object"));
        assert!(
            matches!(
                build_remote_session_request(&scalar, "ios_phone", &stores).expect_err("scalar options"),
                RemoteRequestError::Definition(RemoteConfigError::InvalidCapabilityExtension {
                    problem: ExtensionProblem::NotAnObject,
                    ..
                })
            ),
            "a configured scalar is refused, never overwritten"
        );
    }

    #[test]
    fn unknown_targets_and_unready_credentials_are_named_without_values() {
        let config = config();
        let session = SessionCredentialStore::new();
        let stores = CredentialStores {
            session: &session,
            os_keychain: None,
        };
        assert_eq!(
            build_remote_session_request(&config, "android", &stores).expect_err("unknown"),
            RemoteRequestError::UnknownTarget {
                target: "android".into()
            }
        );
        let mut orphaned = config.clone();
        orphaned.targets.get_mut("ios_phone").expect("target").provider = "nowhere".into();
        assert_eq!(
            build_remote_session_request(&orphaned, "ios_phone", &stores).expect_err("unknown provider"),
            RemoteRequestError::UnknownProvider {
                target: "ios_phone".into(),
                provider: "nowhere".into(),
            },
            "the selected target is looked up before the whole definition is validated"
        );
        let error = build_remote_session_request(&config, "ios_phone", &stores).expect_err("missing user");
        match error {
            RemoteRequestError::Credential { provider, error } => {
                assert_eq!(provider, "grid");
                assert_eq!(error.reference.as_str(), "user");
                assert_eq!(error.error, RemoteCredentialError::Missing);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
