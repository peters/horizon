//! Host-side planning for an agent create that names a configured remote
//! target. The agent supplies only the target name; the host resolves the
//! provider, the capabilities and the authorization from configuration and
//! the credential workbench, and refuses with a typed reason that never
//! carries a credential value.

use horizon_core::Config;
use horizon_core::browser::{
    BackendKind, RemoteRequestError, RemoteSessionRequest, build_remote_session_request, remote_slots,
};
use horizon_core::remote_browser_credential::{
    CredentialStores, CredentialWorkbench, RemoteCredentialError, RemoteCredentialStore,
};

use super::HorizonApp;

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
    pub(super) max_sessions: u32,
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
    let (quota_key, max_sessions) = remote
        .providers
        .get(&provider)
        .map(|profile| (remote_slots::quota_key(profile), profile.limits.max_sessions))
        .unwrap_or_default();
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
        RemoteRequestError::Credential { ref error, .. }
            if matches!(
                error.error,
                RemoteCredentialError::Missing
                    | RemoteCredentialError::Locked
                    | RemoteCredentialError::StoreUnavailable
                    | RemoteCredentialError::Checking
            ) =>
        {
            CreateRefusal {
                code: "credentials_not_ready",
                message: format!("{error}; enter or unlock it in Settings > Remote browsers"),
            }
        }
        RemoteRequestError::Credential { .. } => CreateRefusal {
            code: "credentials_invalid",
            message: format!("{error}; re-enter it in Settings > Remote browsers"),
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
        .map(|profile| profile.limits.max_sessions)
    else {
        return true;
    };
    holds >= usize::try_from(limit).unwrap_or(usize::MAX)
}

impl HorizonApp {
    pub(super) fn plan_and_admit_remote(
        &mut self,
        request: &horizon_core::browser::manifest::BrowserCreateRequest,
        workspace_id: horizon_core::WorkspaceId,
    ) -> Result<Option<RemoteCreatePlan>, CreateRefusal> {
        let Some(target) = request.target.as_deref() else {
            return Ok(None);
        };
        let plan = plan_remote_create(&self.template_config, &self.remote_browser_credentials, target)?;
        let workspace = self
            .board
            .workspace(workspace_id)
            .map(|w| w.local_id.clone())
            .ok_or_else(|| CreateRefusal {
                code: "workspace_unavailable",
                message: "the requesting workspace no longer exists".into(),
            })?;
        self.admit_remote_create(&plan, &request.actor, &workspace)
            .map_err(|(code, message)| CreateRefusal {
                code,
                message: message.to_string(),
            })?;
        Ok(Some(plan))
    }

    /// Admit a planned remote create against the provider's quota: first
    /// this host's own holds, then the slots shared with other Horizon
    /// instances on this computer.
    ///
    /// # Errors
    /// The typed create refusal when the quota is reached either way.
    pub(super) fn admit_remote_create(
        &mut self,
        plan: &RemoteCreatePlan,
        owner: &str,
        workspace: &str,
    ) -> Result<(), (&'static str, &'static str)> {
        self.browser_create_host.remote_allocations.poll();
        let holds = remote_holds(self, &plan.provider);
        if remote_session_limit_reached(&self.template_config, &plan.provider, holds) {
            return Err((
                "remote_session_limit_reached",
                "the remote provider has reached its configured max_sessions; allocations count until their release is established",
            ));
        }
        let lease = self.lease_remote_slot(plan)?;
        self.browser_create_host.remote_allocations.insert(
            horizon_core::browser::remote_recovery::HeldRemoteAllocation {
                allocation: plan.request.recovery.clone(),
                provider: plan.provider.clone(),
                workspace: workspace.to_string(),
                owner: owner.to_string(),
                lease,
            },
        );
        Ok(())
    }

    /// Lease one cross-instance provider slot for the planned create. Other
    /// Horizon instances on this computer share the quota through slot files
    /// under the Horizon home; the slot is leased before anything is
    /// allocated and kept until the release is established. Slot files that
    /// cannot be used are logged, and the host's own count stands alone.
    ///
    /// # Errors
    /// The typed create refusal when every slot is held.
    pub(super) fn lease_remote_slot(
        &mut self,
        plan: &RemoteCreatePlan,
    ) -> Result<Option<remote_slots::SlotLease>, (&'static str, &'static str)> {
        let root = self.session_store.home();
        match remote_slots::acquire_slot(root.root(), &plan.quota_key, plan.max_sessions) {
            Ok(lease) => Ok(Some(lease)),
            Err(remote_slots::SlotError::Busy { .. }) => Err((
                "remote_session_limit_reached",
                "the remote provider's configured max_sessions are held by Horizon instances on this computer; allocations count until their release is established",
            )),
            Err(remote_slots::SlotError::Contended) => Err((
                "remote_quota_contended",
                "another Horizon instance on this computer was checking the same provider quota; create again",
            )),
            Err(error) => {
                tracing::warn!(%error, "remote provider slot files unavailable; proceeding on this host's count alone");
                Ok(None)
            }
        }
    }
}

/// Free only leases whose exact allocation is released, and prune old board holds.
pub(super) fn trim_remote_slot_leases(app: &mut HorizonApp) {
    app.browser_create_host.remote_allocations.poll();
    app.browser_create_host.orphaned_remote_holds.retain(|hold| {
        !hold
            .recovery
            .as_ref()
            .is_some_and(horizon_core::browser::RemoteAllocation::is_released)
    });
}

impl HorizonApp {
    /// Take over the unreleased holds of a shutdown that is ending, so they
    /// keep counting against their provider and their slots stay leased
    /// after the board is gone.
    pub(super) fn adopt_orphaned_remote_holds(&mut self, progress: &horizon_core::ShutdownProgress) {
        self.browser_create_host
            .orphaned_remote_holds
            .extend(progress.take_unreleased_remote_holds());
    }
}

/// Allocations `provider` may still hold anywhere this host knows about:
/// the board's live and retired remote sessions, plus closes the host is
/// still waiting on. Each is counted until its release is established, and
/// the provider identity travels with the session, not with the target
/// configuration, so re-pointing a target never uncounts an allocation.
pub(super) fn remote_holds(app: &HorizonApp, provider: &str) -> usize {
    let pending = app
        .browser_create_host
        .pending_closes
        .iter()
        .filter(|pending| pending.holds_remote_allocation_at(provider))
        .count();
    let orphaned = app
        .browser_create_host
        .orphaned_remote_holds
        .iter()
        .filter(|held| {
            held.provider == provider
                && !held
                    .recovery
                    .as_ref()
                    .is_some_and(horizon_core::browser::RemoteAllocation::is_released)
        })
        .count();
    app.board.remote_holds(provider) + pending + orphaned
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
    fn a_cancelled_admission_frees_its_lease_before_the_next_create_in_the_batch() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.template_config = config();
        let mut credentials = workbench();
        credentials
            .set_session_value(
                "grid",
                &app.template_config.browser.remote.providers["grid"],
                &CredentialReference::from("key"),
                b"fixture-token",
            )
            .expect("credential");
        let first = plan_remote_create(&app.template_config, &credentials, "ios_phone").expect("first plan");
        app.admit_remote_create(&first, "owner", "workspace")
            .expect("first admission");
        first.request.recovery.cancel_before_launch();
        assert!(matches!(
            remote_slots::acquire_slot(app.session_store.home().root(), &first.quota_key, 1),
            Err(remote_slots::SlotError::Busy { .. })
        ));
        let second = plan_remote_create(&app.template_config, &credentials, "ios_phone").expect("second plan");
        app.admit_remote_create(&second, "owner", "workspace")
            .expect("same-batch admission without a host poll");
        assert!(
            matches!(
                remote_slots::acquire_slot(app.session_store.home().root(), &second.quota_key, 1),
                Err(remote_slots::SlotError::Busy { .. })
            ),
            "the new allocation keeps its lease"
        );
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

    #[test]
    fn malformed_credentials_are_invalid_not_merely_missing() {
        let config = config();
        let mut workbench = workbench();
        let profile = &config.browser.remote.providers["grid"];
        workbench
            .set_session_value("grid", profile, &CredentialReference::from("key"), b"has space")
            .expect("session value");
        let refused = plan_remote_create(&config, &workbench, "ios_phone").expect_err("token grammar");
        assert_eq!(refused.code, "credentials_invalid");
        assert!(refused.message.contains("`key`"));
        assert!(!refused.message.contains("has space"), "no value leaks");
    }
}
