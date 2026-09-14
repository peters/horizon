//! Host-side planning for an agent create that names a configured remote
//! target. The agent supplies only the target name; the host resolves the
//! provider, the capabilities and the authorization from configuration and
//! the credential workbench, and refuses with a typed reason that never
//! carries a credential value.

use horizon_core::browser::{BackendKind, RemoteRequestError, RemoteSessionRequest, build_remote_session_request};
use horizon_core::remote_browser_credential::{CredentialStores, CredentialWorkbench, RemoteCredentialStore};
use horizon_core::{Board, Config};

/// Everything the host needs to open a panel at a remote target.
#[derive(Debug)]
pub(super) struct RemoteCreatePlan {
    pub(super) request: RemoteSessionRequest,
    /// The browser family the target drives (from the request), for the
    /// panel's capabilities and audit; the driver runs classic `WebDriver`
    /// regardless.
    pub(super) backend: BackendKind,
    pub(super) provider: String,
}

/// Why a remote create is refused: the typed result code and its message.
#[derive(Debug)]
pub(super) struct CreateRefusal {
    pub(super) code: &'static str,
    pub(super) message: String,
}

/// Resolve `target` into a session request now, so allocation later never
/// touches a credential store and a missing or locked credential is
/// reported to the agent before any panel exists.
pub(super) fn plan_remote_create(
    config: &Config,
    workbench: &CredentialWorkbench,
    target: &str,
) -> Result<RemoteCreatePlan, CreateRefusal> {
    let remote = &config.browser.remote;
    let Some(profile) = remote.targets.get(target) else {
        return Err(CreateRefusal {
            code: "target_unknown",
            message: format!("remote target `{target}` is not configured in browser.remote.targets"),
        });
    };
    let provider = profile.provider.clone();
    let keychain = workbench.keychain_store();
    let keychain_guard = keychain
        .as_ref()
        .map(|store| store.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let stores = CredentialStores {
        session: workbench.session_store(),
        os_keychain: keychain_guard
            .as_deref()
            .map(|store| -> &dyn RemoteCredentialStore { &**store }),
    };
    let request = build_remote_session_request(remote, target, &stores).map_err(|error| match error {
        RemoteRequestError::UnknownTarget { .. } => CreateRefusal {
            code: "target_unknown",
            message: error.to_string(),
        },
        RemoteRequestError::UnknownProvider { .. } | RemoteRequestError::Definition(_) => CreateRefusal {
            code: "target_invalid",
            message: error.to_string(),
        },
        RemoteRequestError::Credential { .. } => CreateRefusal {
            code: "credentials_not_ready",
            message: format!("{error}; enter or unlock it in Settings > Remote browsers"),
        },
        RemoteRequestError::Header { .. } => CreateRefusal {
            code: "credentials_invalid",
            message: error.to_string(),
        },
    })?;
    // The builder already fixed the browser family on the request; it is
    // the panel's backend and the family the audit records.
    let backend = request.browser;
    Ok(RemoteCreatePlan {
        request,
        backend,
        provider,
    })
}

/// Whether the provider's configured `max_sessions` is already used by live
/// remote panels on this board, counted through each panel's target.
pub(super) fn remote_session_limit_reached(board: &Board, config: &Config, provider: &str) -> bool {
    let remote = &config.browser.remote;
    let Some(limit) = remote
        .providers
        .get(provider)
        .map(|profile| profile.limits.max_sessions)
    else {
        return true;
    };
    let live = board
        .panels
        .iter()
        .filter_map(|panel| panel.browser())
        .filter(|browser| browser.status.is_alive())
        .filter_map(|browser| browser.remote_target())
        .filter(|target| {
            remote
                .targets
                .get(*target)
                .is_some_and(|profile| profile.provider == provider)
        })
        .count();
    live >= usize::try_from(limit).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits, RemoteTargetProfile,
    };
    use horizon_core::remote_browser_credential::{FakeCredentialStore, RemoteCredentialError, RemoteCredentialStore};

    use super::*;

    fn config() -> Config {
        let mut config = Config::default();
        let mut bindings = BTreeMap::new();
        bindings.insert(
            CredentialReference::from("key"),
            CredentialBinding {
                store: CredentialStoreKind::Session,
                slot: None,
            },
        );
        config.browser.remote.providers.insert(
            "grid".to_string(),
            RemoteProviderProfile {
                adapter: RemoteAdapterKind::Webdriver,
                endpoint: ControlEndpoint::parse("https://grid.example.net/wd/hub").expect("endpoint"),
                authentication: RemoteAuthentication::Bearer {
                    token_ref: CredentialReference::from("key"),
                },
                credential_bindings: bindings,
                limits: RemoteSessionLimits {
                    max_sessions: 1,
                    ..RemoteSessionLimits::default()
                },
            },
        );
        config.browser.remote.targets.insert(
            "ios_phone".to_string(),
            RemoteTargetProfile {
                provider: "grid".into(),
                browser_name: "Safari".into(),
                platform_name: "iOS".into(),
                device: horizon_core::browser::remote::DeviceRequirement::default(),
                capability_extensions: BTreeMap::new(),
            },
        );
        config
    }

    fn workbench() -> CredentialWorkbench {
        CredentialWorkbench::with_opener(Box::new(|| {
            Ok(Box::new(FakeCredentialStore::new()) as Box<dyn RemoteCredentialStore + Send>)
        }))
    }

    #[test]
    fn a_configured_target_with_a_ready_credential_becomes_a_plan() {
        let config = config();
        let mut workbench = workbench();
        let profile = &config.browser.remote.providers["grid"];
        workbench
            .set_session_value("grid", profile, &CredentialReference::from("key"), b"tok-en")
            .expect("session value");
        let plan = plan_remote_create(&config, &workbench, "ios_phone").unwrap_or_else(|refused| {
            panic!("{}: {}", refused.code, refused.message);
        });
        assert_eq!(plan.provider, "grid");
        assert_eq!(plan.backend, BackendKind::SafariWebDriver);
        assert_eq!(plan.request.label, "ios_phone");
        assert_eq!(plan.request.endpoint, "https://grid.example.net/wd/hub");
        assert!(plan.request.authorization.is_some());
        assert!(!format!("{:?}", plan.request).contains("tok-en"));
    }

    #[test]
    fn refusals_name_the_target_or_reference_and_never_a_value() {
        let config = config();
        let workbench = workbench();
        let unknown = plan_remote_create(&config, &workbench, "android").expect_err("unknown target");
        assert_eq!(unknown.code, "target_unknown");
        assert!(unknown.message.contains("android"));
        let not_ready = plan_remote_create(&config, &workbench, "ios_phone").expect_err("missing credential");
        assert_eq!(not_ready.code, "credentials_not_ready");
        assert!(not_ready.message.contains("`key`"));
        assert!(not_ready.message.contains(&RemoteCredentialError::Missing.to_string()));
        assert!(not_ready.message.contains("Settings"));
    }

    #[test]
    fn provider_limits_count_live_remote_panels_only() {
        let config = config();
        let board = Board::new();
        assert!(
            !remote_session_limit_reached(&board, &config, "grid"),
            "nothing live yet"
        );
        assert!(
            remote_session_limit_reached(&board, &config, "nowhere"),
            "an unknown provider has no budget"
        );
    }
}
