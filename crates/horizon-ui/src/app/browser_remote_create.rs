//! Host-side planning for an agent create that names a configured remote
//! target. The agent supplies only the target name; the host resolves the
//! provider, the capabilities and the authorization from configuration and
//! the credential workbench, and refuses with a typed reason that never
//! carries a credential value.
//!
//! The plan reads the in-memory stores only. The values that only the OS store
//! holds are read on the workbench thread before it runs
//! ([`browser_remote_credentials`](super::browser_remote_credentials)), and the
//! provider slot is leased on the coordination worker
//! ([`browser_remote_slots`](super::browser_remote_slots)), so a locked store or
//! a slot another Horizon instance holds never stalls a window.

use horizon_core::Config;
use horizon_core::browser::provider_catalog::CatalogDevice;
use horizon_core::browser::remote::RemoteBrowserConfig;
use horizon_core::browser::remote_slots;
use horizon_core::browser::{BackendKind, RemoteRequestError, RemoteSessionRequest, build_remote_session_request};
use horizon_core::remote_browser_credential::{
    CredentialStores, CredentialWorkbench, ReadCredentialStore, RemoteCredentialError, RemoteCredentialStore,
    ResolveError,
};

use super::HorizonApp;
use super::browser_create_lifecycle::PendingBrowserCreate;
use super::browser_request_claims::fail_create;

/// The target name a catalog device is inserted under, so one plan path
/// resolves a configured target and a discovered device alike.
const CATALOG_SELECTION_TARGET: &str = "catalog_selection";

/// Everything the host needs to open a panel at a remote target.
#[derive(Debug)]
pub(super) struct RemoteCreatePlan {
    pub(super) request: RemoteSessionRequest,
    /// The browser family the target drives (from the request), for the
    /// panel's capabilities and audit; the driver runs classic `WebDriver`
    /// regardless.
    pub(super) backend: BackendKind,
    pub(super) provider: String,
    /// The provider identity every Horizon instance on this computer shares
    /// the quota through (see `remote_slots`), and that quota.
    pub(super) quota_key: String,
    pub(super) max_sessions: Option<u32>,
}

/// Why a remote create is refused: the typed result code and its message.
#[derive(Debug)]
pub(super) struct CreateRefusal {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl CreateRefusal {
    /// The refusal of a credential that could not be resolved. The same code
    /// and text describe a value the request resolved directly and one the
    /// workbench read, so where the value was read never changes the answer.
    pub(super) fn for_credential(error: &ResolveError) -> Self {
        if matches!(
            error.error,
            RemoteCredentialError::Missing
                | RemoteCredentialError::Locked
                | RemoteCredentialError::StoreUnavailable
                | RemoteCredentialError::Checking
        ) {
            Self {
                code: "credentials_not_ready",
                message: format!(
                    "{error}; check the selected credential source in Settings > Remote browsers; environment bindings require the named launch variable"
                ),
            }
        } else {
            Self {
                code: "credentials_invalid",
                message: format!(
                    "{error}; correct the selected credential source; environment bindings require a valid named launch variable"
                ),
            }
        }
    }
}

/// Resolve `target` into a session request, so allocation later never touches a
/// credential store and a missing or locked credential is reported to the agent
/// before any panel exists. The session and environment stores are in memory; a
/// value that only the OS store holds arrives in `os_values`, read on the
/// workbench thread, so this never takes the store mutex or asks the keychain.
pub(super) fn plan_remote_create(
    remote: &RemoteBrowserConfig,
    workbench: &CredentialWorkbench,
    os_values: Option<&ReadCredentialStore>,
    target: &str,
) -> Result<RemoteCreatePlan, CreateRefusal> {
    let Some(profile) = remote.targets.get(target) else {
        return Err(CreateRefusal {
            code: "target_unknown",
            message: format!("remote target `{target}` is not configured in browser.remote.targets"),
        });
    };
    let provider = profile.provider.clone();
    let (quota_key, max_sessions) = remote
        .providers
        .get(&provider)
        .map(|profile| (remote_slots::quota_key(profile), profile.local_session_limit()))
        .unwrap_or_default();
    let stores = CredentialStores {
        session: workbench.session_store(),
        os_keychain: os_values.map(|values| -> &dyn RemoteCredentialStore { values }),
        environment: Some(workbench.environment_store()),
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
        RemoteRequestError::Credential { error, .. } => CreateRefusal::for_credential(&error),
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
        quota_key,
        max_sessions,
    })
}

/// Whether `holds` allocations already use up the provider's configured
/// `max_sessions`. An unknown provider has no budget.
pub(super) fn remote_session_limit_reached(config: &Config, provider: &str, holds: usize) -> bool {
    let Some(limit) = config
        .browser
        .remote
        .providers
        .get(provider)
        .map(horizon_core::browser::remote::RemoteProviderProfile::local_session_limit)
    else {
        return true;
    };
    limit.is_some_and(|limit| holds >= usize::try_from(limit).unwrap_or(usize::MAX))
}

/// The remote configuration a create is planned against, and the target name
/// in it: a configured target, or the catalog device the agent named.
pub(super) struct RemoteTarget {
    /// The configuration, carrying a catalog device as the
    /// [`CATALOG_SELECTION_TARGET`] target when the agent named one.
    pub(super) remote: RemoteBrowserConfig,
    /// The target name in `remote`.
    pub(super) name: String,
    /// The device a catalog target named, for the request's options.
    pub(super) catalog: Option<CatalogDevice>,
}

impl HorizonApp {
    /// The configuration a create is planned against: the configured target,
    /// or the catalog device the request named turned into one target.
    pub(super) fn remote_target(&mut self, target: &str) -> Result<RemoteTarget, CreateRefusal> {
        let Some(provider) = horizon_core::browser::remote_catalog::target_provider(target) else {
            return Ok(RemoteTarget {
                remote: self.template_config.browser.remote.clone(),
                name: target.to_string(),
                catalog: None,
            });
        };
        let profile = self
            .template_config
            .browser
            .remote
            .providers
            .get(provider)
            .ok_or_else(|| CreateRefusal {
                code: "provider_unknown",
                message: "Catalog provider is not configured".into(),
            })?;
        self.browser_create_host
            .catalog
            .cache
            .invalidate_credentials(self.remote_browser_credentials.generation());
        let device = self
            .browser_create_host
            .catalog
            .cache
            .target(profile, target)
            .cloned()
            .map_err(|error| CreateRefusal {
                code: "provider_catalog_refresh_required",
                message: error.to_string(),
            })?;
        let mut remote = self.template_config.browser.remote.clone();
        remote.targets.insert(
            CATALOG_SELECTION_TARGET.into(),
            horizon_core::browser::provider_catalog::target_profile(&device),
        );
        Ok(RemoteTarget {
            remote,
            name: CATALOG_SELECTION_TARGET.into(),
            catalog: Some(device),
        })
    }

    /// Answers a remote create that never opened its panel with its typed
    /// refusal. No panel exists, so no dispatch audit is written either, and the
    /// credential values it held are wiped as it drops.
    pub(super) fn refuse_remote_create(&mut self, pending: &PendingBrowserCreate, refusal: &CreateRefusal) {
        fail_create(
            &mut self.browser_create_host.io,
            &pending.request,
            refusal.code,
            &refusal.message,
        );
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::collections::BTreeMap;

    use horizon_core::browser::remote::{
        ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
        RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits, RemoteTargetProfile,
    };
    use horizon_core::remote_browser_credential::{FakeCredentialStore, RemoteCredentialError, RemoteCredentialStore};

    use super::*;

    pub(in crate::app) fn config() -> Config {
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
                orientation: None,
                capability_extensions: BTreeMap::new(),
            },
        );
        config
    }

    pub(in crate::app) fn workbench() -> CredentialWorkbench {
        CredentialWorkbench::with_opener(Box::new(|| {
            Ok(Box::new(FakeCredentialStore::new()) as Box<dyn RemoteCredentialStore + Send>)
        }))
    }

    #[test]
    fn same_binding_credential_change_fences_a_previously_discovered_target() {
        use horizon_core::browser::remote_catalog::{CatalogDevice, CatalogQuery};
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.template_config = config();
        let profile = app.template_config.browser.remote.providers["grid"].clone();
        let reference = CredentialReference::from("key");
        app.remote_browser_credentials
            .set_session_value("grid", &profile, &reference, b"original")
            .unwrap();
        let target = format!("catalog.grid.{}", "a".repeat(64));
        let row = CatalogDevice {
            target: target.clone(),
            provider: "grid".into(),
            os: "ios".into(),
            os_version: "18".into(),
            browser: "iphone".into(),
            browser_version: None,
            device: Some("Synthetic phone".into()),
            real_mobile: true,
        };
        let cache = &mut app.browser_create_host.catalog.cache;
        cache.invalidate_credentials(app.remote_browser_credentials.generation());
        cache.start("grid", &profile, move |_| Ok(vec![row]));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let query = CatalogQuery {
            provider: "grid".into(),
            ..Default::default()
        };
        while cache.page(&profile, &query).unwrap().is_none() {
            cache.poll();
            assert!(std::time::Instant::now() < end);
            std::thread::yield_now();
        }
        app.remote_browser_credentials
            .set_session_value("grid", &profile, &reference, b"replacement")
            .unwrap();
        let Err(error) = app.remote_target(&target) else {
            panic!("stale catalog admitted after credential replacement")
        };
        assert_eq!(error.code, "provider_catalog_refresh_required");
        assert!(app.browser_create_host.catalog.cache.needs_refresh("grid", &profile));
    }

    #[test]
    fn a_configured_target_with_a_ready_credential_becomes_a_plan() {
        let config = config();
        let mut workbench = workbench();
        let profile = &config.browser.remote.providers["grid"];
        workbench
            .set_session_value("grid", profile, &CredentialReference::from("key"), b"tok-en")
            .expect("session value");
        let plan =
            plan_remote_create(&config.browser.remote, &workbench, None, "ios_phone").unwrap_or_else(|refused| {
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
        let unknown =
            plan_remote_create(&config.browser.remote, &workbench, None, "android").expect_err("unknown target");
        assert_eq!(unknown.code, "target_unknown");
        assert!(unknown.message.contains("android"));
        let not_ready =
            plan_remote_create(&config.browser.remote, &workbench, None, "ios_phone").expect_err("missing credential");
        assert_eq!(not_ready.code, "credentials_not_ready");
        assert!(not_ready.message.contains("`key`"));
        assert!(not_ready.message.contains(&RemoteCredentialError::Missing.to_string()));
        assert!(not_ready.message.contains("Settings"));
    }

    #[test]
    fn provider_limits_count_held_allocations_until_release_is_established() {
        use horizon_core::browser::{BrowserPanelState, BrowserShutdownSignal, RemoteReleaseOutcome};
        use horizon_core::{Board, Panel, PanelContent, PanelId, PanelKind, WorkspaceId};

        let config = config();
        let mut board = Board::new();
        let workspace: WorkspaceId = board.create_workspace("alpha");
        assert_eq!(board.remote_holds("grid"), 0);
        assert!(!remote_session_limit_reached(&config, "grid", 0), "nothing held yet");
        assert!(
            remote_session_limit_reached(&config, "nowhere", 0),
            "an unknown provider has no budget"
        );

        let mut push = |id: u64, state: BrowserPanelState| {
            let panel = Panel::from_content(
                PanelId(id),
                workspace,
                PanelKind::Browser,
                PanelContent::Browser(Box::new(state)),
            );
            board.panels.push(panel);
        };
        push(9101, BrowserPanelState::inert_remote("ios_phone", "grid"));
        push(9102, BrowserPanelState::inert_remote("pixel", "other-grid"));
        push(
            9103,
            BrowserPanelState::restored_remote(
                "restored",
                &horizon_core::browser::BrowserConfig::default(),
                "ios_phone".into(),
                None,
            ),
        );
        let mut released = BrowserPanelState::inert_remote("ios_phone", "grid");
        released.mark_remote_release_established_for_tests();
        push(9104, released);
        let mut stopped = BrowserPanelState::inert_remote("ios_phone", "grid");
        stopped.stop();
        push(9105, stopped);
        assert_eq!(
            board.remote_holds("grid"),
            2,
            "one live and one stopped-but-unreleased session count; the other provider, the restored panel and the released session do not"
        );
        assert!(
            remote_session_limit_reached(&config, "grid", board.remote_holds("grid")),
            "max_sessions is 1"
        );
        assert_eq!(board.remote_holds("other-grid"), 1);

        // A teardown that finished without an established release keeps
        // counting after the board dropped it; an established one does not.
        board.retire_browser_shutdown_signal(BrowserShutdownSignal::completed_remote_for_test(
            "grid",
            Some(RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "timed out".to_string(),
            }),
        ));
        board.retire_browser_shutdown_signal(BrowserShutdownSignal::completed_remote_for_test("grid", None));
        board.retire_browser_shutdown_signal(BrowserShutdownSignal::completed_remote_for_test(
            "grid",
            Some(RemoteReleaseOutcome::Released),
        ));
        board.retire_browser_shutdown_signal(BrowserShutdownSignal::completed_remote_for_test(
            "other-grid",
            Some(RemoteReleaseOutcome::AlreadyGone),
        ));
        let before = board.remote_holds("grid");
        let _ = board.process_output();
        assert!(
            !board.has_pending_browser_cleanup(),
            "finished teardowns stop the polling"
        );
        assert_eq!(
            board.remote_holds("grid"),
            before,
            "sweeping finished teardowns changes no count"
        );
        assert_eq!(
            board.remote_holds("grid"),
            2 + 2,
            "unknown and unreported releases keep counting"
        );
        assert_eq!(
            board.remote_holds("other-grid"),
            1,
            "an established release frees the slot"
        );
    }
}
