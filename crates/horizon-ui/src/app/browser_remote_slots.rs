//! The provider slot of a remote create. The slot files are shared with every
//! Horizon instance on this computer, and taking one may wait on a lock, so the
//! lease runs on the coordination worker: a create reaches its panel only after
//! the lease applies. Each lease is recorded with the create's allocation, which
//! counts against its provider until the release is established.

use horizon_core::browser::provider_catalog::apply_catalog_options;
use horizon_core::browser::remote_config::override_orientation;
use horizon_core::browser::remote_recovery::HeldRemoteAllocation;
use horizon_core::browser::remote_slots::{self, SlotError, SlotLease};
use horizon_core::remote_browser_credential::ReadCredentialStore;

use super::HorizonApp;
use super::browser_create_lifecycle::{CreateStage, PendingBrowserCreate};
use super::browser_remote_create::{
    CreateRefusal, RemoteCreatePlan, RemoteTarget, plan_remote_create, remote_session_limit_reached,
};
use super::browser_remote_credentials::RemoteCreateStep;
use super::browser_request_claims::HOST_RETIRING;
use super::browser_requests::ActorPanel;

impl HorizonApp {
    /// Plans the request from the values at hand and leases its provider slot:
    /// on the coordination worker when the provider has a local quota, and at
    /// once when it manages its own capacity. The panel of the create opens
    /// only after the lease applies.
    pub(super) fn plan_and_lease_remote(
        &mut self,
        mut pending: PendingBrowserCreate,
        actor_panel: ActorPanel,
        target: &RemoteTarget,
        values: &ReadCredentialStore,
    ) -> RemoteCreateStep {
        let mut plan = match plan_remote_create(
            &target.remote,
            &self.remote_browser_credentials,
            (!values.is_empty()).then_some(values),
            &target.name,
        ) {
            Ok(plan) => plan,
            Err(refusal) => {
                self.refuse_remote_create(&pending, &refusal);
                return None;
            }
        };
        if let Some(device) = &target.catalog {
            apply_catalog_options(&mut plan.request, device);
        }
        override_orientation(&mut plan.request, pending.request.orientation);
        let Some(workspace) = self
            .board
            .workspace(actor_panel.workspace_id)
            .map(|workspace| workspace.local_id.clone())
        else {
            let refusal = CreateRefusal {
                code: "workspace_unavailable",
                message: "the requesting workspace no longer exists".into(),
            };
            self.refuse_remote_create(&pending, &refusal);
            return None;
        };
        if let Err((code, message)) = self.remote_admission_refusal(&plan) {
            let refusal = CreateRefusal {
                code,
                message: message.into(),
            };
            self.refuse_remote_create(&pending, &refusal);
            return None;
        }
        let Some(max_sessions) = plan.max_sessions else {
            // The provider manages its own capacity: there is no local slot to lease.
            self.record_remote_allocation(&plan, &pending.request.actor, &workspace, None);
            self.open_remote_create_panel(pending, actor_panel, plan);
            return None;
        };
        let root = self.session_store.home().root().to_path_buf();
        let quota_key = plan.quota_key.clone();
        let request_id = pending.request.request_id.clone();
        let leased = (plan, workspace);
        self.browser_create_host.io.then(
            move || remote_slots::acquire_slot(&root, &quota_key, max_sessions),
            move |app, lease| app.apply_remote_slot_lease(&request_id, actor_panel, leased, lease),
        );
        pending.stage = CreateStage::Leasing;
        Some(pending)
    }

    /// Takes the lease the coordination worker acquired for a create that still
    /// waits for it. A create that ended meanwhile, or whose board was replaced
    /// or retired, drops the lease here instead of recording it, and dropping a
    /// lease frees its slot for the next allocation on this computer.
    fn apply_remote_slot_lease(
        &mut self,
        request_id: &str,
        actor_panel: ActorPanel,
        leased: (RemoteCreatePlan, String),
        lease: Option<Result<SlotLease, SlotError>>,
    ) {
        let stale = self.browser_create_host.io.applying_stale();
        let found = self
            .browser_create_host
            .pending
            .iter()
            .position(|pending| pending.request.request_id == request_id && pending.stage == CreateStage::Leasing);
        // Without a create to give it to, `lease` drops at the end of this call.
        let Some(index) = found else { return };
        if stale || self.browser_host_retiring() {
            let pending = self.browser_create_host.pending.remove(index);
            let refusal = CreateRefusal {
                code: HOST_RETIRING.0,
                message: HOST_RETIRING.1.into(),
            };
            self.refuse_remote_create(&pending, &refusal);
            return;
        }
        let (plan, workspace) = leased;
        let lease = match lease {
            Some(Ok(lease)) => Some(lease),
            // The lease job panicked: no slot is held, as after an unusable
            // slot directory, and this host's own count stands alone.
            None => {
                tracing::error!(%request_id, "the remote provider slot lease job failed");
                None
            }
            Some(Err(SlotError::Busy { .. })) => {
                let pending = self.browser_create_host.pending.remove(index);
                let refusal = CreateRefusal {
                    code: "remote_session_limit_reached",
                    message: "the remote provider's configured max_sessions are held by Horizon instances on this computer; allocations count until their release is established".into(),
                };
                self.refuse_remote_create(&pending, &refusal);
                return;
            }
            Some(Err(SlotError::Contended)) => {
                let pending = self.browser_create_host.pending.remove(index);
                let refusal = CreateRefusal {
                    code: "remote_quota_contended",
                    message:
                        "another Horizon instance on this computer was checking the same provider quota; create again"
                            .into(),
                };
                self.refuse_remote_create(&pending, &refusal);
                return;
            }
            Some(Err(error)) => {
                tracing::warn!(%error, "remote provider slot files unavailable; proceeding on this host's count alone");
                None
            }
        };
        let pending = self.browser_create_host.pending.remove(index);
        self.record_remote_allocation(&plan, &pending.request.actor, &workspace, lease);
        self.open_remote_create_panel(pending, actor_panel, plan);
    }

    /// Refuses a planned remote create when this host already holds a session
    /// at the provider's configured limit, before a shared slot is asked for.
    ///
    /// # Errors
    /// The typed create refusal when the quota is reached.
    fn remote_admission_refusal(&mut self, plan: &RemoteCreatePlan) -> Result<(), (&'static str, &'static str)> {
        self.browser_create_host.remote_allocations.poll();
        let holds = remote_holds(self, &plan.provider);
        if remote_session_limit_reached(&self.template_config, &plan.provider, holds) {
            return Err((
                "remote_session_limit_reached",
                "the remote provider has reached its configured max_sessions; allocations count until their release is established",
            ));
        }
        Ok(())
    }

    /// Records a planned create's allocation against its provider, holding
    /// `lease` until the release is established. The allocation poll frees the
    /// slot once the allocation reports its release, and a refused create that
    /// never launched cancels it (`cancel_before_launch`), so a refusal never
    /// keeps a slot.
    fn record_remote_allocation(
        &mut self,
        plan: &RemoteCreatePlan,
        owner: &str,
        workspace: &str,
        lease: Option<SlotLease>,
    ) {
        self.browser_create_host
            .remote_allocations
            .insert(HeldRemoteAllocation {
                allocation: plan.request.recovery.clone(),
                provider: plan.provider.clone(),
                workspace: workspace.to_string(),
                owner: owner.to_string(),
                lease,
            });
    }

    /// Opens the panel of a remote create whose allocation is recorded. From
    /// here the create takes the same path as a local one: its panel, its
    /// dispatch audit and its result.
    fn open_remote_create_panel(
        &mut self,
        pending: PendingBrowserCreate,
        actor_panel: ActorPanel,
        plan: RemoteCreatePlan,
    ) {
        let mut browser_config = self.template_config.browser.clone();
        browser_config.backend = plan.backend;
        let options = horizon_core::PanelOptions {
            command: pending.request.url.clone(),
            kind: horizon_core::PanelKind::Browser,
            visible: pending.request.visible,
            browser_config: Some(browser_config),
            remote_session: Some(plan.request),
            ..horizon_core::PanelOptions::default()
        };
        self.open_requested_browser_panel(pending.request, actor_panel, options, pending.started_at);
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

#[cfg(test)]
mod tests {
    use horizon_core::browser::remote::{CredentialReference, RemoteAdapterKind};

    use super::*;
    use crate::app::browser_remote_create::tests::{config, workbench};

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
        let first = plan_remote_create(&app.template_config.browser.remote, &credentials, None, "ios_phone")
            .expect("first plan");
        app.remote_admission_refusal(&first).expect("first admission");
        let lease =
            remote_slots::acquire_slot(app.session_store.home().root(), &first.quota_key, 1).expect("first slot");
        app.record_remote_allocation(&first, "owner", "workspace", Some(lease));
        first.request.recovery.cancel_before_launch();
        assert!(
            matches!(
                remote_slots::acquire_slot(app.session_store.home().root(), &first.quota_key, 1),
                Err(remote_slots::SlotError::Busy { .. })
            ),
            "a cancelled allocation keeps its lease until the host polls"
        );
        let second = plan_remote_create(&app.template_config.browser.remote, &credentials, None, "ios_phone")
            .expect("second plan");
        // Concurrent process tests can inherit the freed lock until exec.
        // Retry the admission only; an omitted admission-time poll still fails.
        (0..100)
            .find_map(|_| {
                app.remote_admission_refusal(&second).ok()?;
                match remote_slots::acquire_slot(app.session_store.home().root(), &second.quota_key, 1) {
                    Ok(lease) => Some(lease),
                    Err(remote_slots::SlotError::Busy { .. }) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        None
                    }
                    Err(error) => panic!("unexpected slot failure: {error:?}"),
                }
            })
            .map(|lease| app.record_remote_allocation(&second, "owner", "workspace", Some(lease)))
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
    fn provider_managed_capacity_bypasses_local_holds_and_locks_but_tracks_ownership() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.template_config = config();
        app.template_config
            .browser
            .remote
            .providers
            .get_mut("grid")
            .expect("provider")
            .adapter = RemoteAdapterKind::Browserstack;
        let mut credentials = workbench();
        credentials
            .set_session_value(
                "grid",
                &app.template_config.browser.remote.providers["grid"],
                &CredentialReference::from("key"),
                b"synthetic-key",
            )
            .expect("credential");
        assert!(!remote_session_limit_reached(&app.template_config, "grid", usize::MAX));
        let first =
            plan_remote_create(&app.template_config.browser.remote, &credentials, None, "ios_phone").expect("first");
        let second =
            plan_remote_create(&app.template_config.browser.remote, &credentials, None, "ios_phone").expect("second");
        assert!(
            first.max_sessions.is_none(),
            "the provider manages its own capacity, so no local slot is leased"
        );
        let _existing_lock = remote_slots::acquire_slot(app.session_store.home().root(), &first.quota_key, 1)
            .expect("existing instance");
        app.remote_admission_refusal(&first).expect("first admission");
        app.remote_admission_refusal(&second).expect("second admission");
        app.record_remote_allocation(&first, "owner-a", "workspace", None);
        app.record_remote_allocation(&second, "owner-b", "workspace", None);
        let records = &mut app.browser_create_host.remote_allocations;
        assert_eq!(records.summaries(None).len(), 2);
        assert_eq!(records.summaries(Some(("owner-a", "workspace"))).len(), 1);
        assert!(!records.reconcile(second.request.recovery.reference(), Some(("owner-a", "workspace"))));
        first.request.recovery.cancel_before_launch();
        let released = records.summaries(Some(("owner-a", "workspace")));
        assert_eq!(
            released[0].status,
            horizon_core::browser::RemoteRecoveryStatus::Released
        );
        assert_ne!(
            records.summaries(Some(("owner-b", "workspace")))[0].status,
            horizon_core::browser::RemoteRecoveryStatus::Released
        );
    }

    #[test]
    fn a_remote_create_opens_no_panel_until_the_worker_leased_its_slot() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.template_config = config();
        let profile = app.template_config.browser.remote.providers["grid"].clone();
        app.remote_browser_credentials
            .set_session_value("grid", &profile, &CredentialReference::from("key"), b"tok-en")
            .expect("session value");
        let workspace = app.board.create_workspace("alpha");
        let mut request = horizon_core::browser::manifest::BrowserCreateRequest::for_tests("agent");
        request.target = Some("ios_phone".into());
        let actor = ActorPanel {
            panel_id: horizon_core::PanelId(1),
            workspace_id: workspace,
        };
        let panels = app.board.panels.len();
        // The worker waits, as it does for a slot another Horizon instance holds.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        let started = std::time::Instant::now();
        app.begin_remote_create(request, actor, std::time::Instant::now());
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the provider slot"
        );
        assert!(
            app.browser_create_host.io.busy(),
            "the lease runs on the coordination worker"
        );
        assert_eq!(
            app.browser_create_host.pending[0].stage,
            CreateStage::Leasing,
            "the create waits for its lease, not for a panel"
        );
        assert_eq!(
            app.board.panels.len(),
            panels,
            "no panel opens before its provider slot is leased"
        );

        // A create that ended takes no lease and opens no panel.
        app.browser_create_host.pending.clear();
        release.send(()).expect("release the coordination worker");
        app.settle_browser_host_io();
        assert!(app.browser_create_host.pending.is_empty());
        assert_eq!(app.board.panels.len(), panels);
        assert!(
            app.browser_create_host.remote_allocations.summaries(None).is_empty(),
            "the ended create records no allocation"
        );
    }

    #[test]
    fn a_lease_that_applies_to_no_create_is_not_recorded_and_frees_its_slot() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.template_config = config();
        let profile = app.template_config.browser.remote.providers["grid"].clone();
        let key = CredentialReference::from("key");
        app.remote_browser_credentials
            .set_session_value("grid", &profile, &key, b"tok-en")
            .expect("session value");
        let root = app.session_store.home().root().to_path_buf();
        let quota = remote_slots::quota_key(&profile);
        let plan = plan_remote_create(
            &app.template_config.browser.remote,
            &app.remote_browser_credentials,
            None,
            "ios_phone",
        )
        .expect("plan");
        let lease = remote_slots::acquire_slot(&root, &quota, 1).expect("the slot is free");
        let actor = ActorPanel {
            panel_id: horizon_core::PanelId(1),
            workspace_id: app.board.create_workspace("alpha"),
        };

        // The create that asked for it ended before the lease applied.
        app.apply_remote_slot_lease("no-create-holds-this", actor, (plan, "alpha".into()), Some(Ok(lease)));

        assert!(
            app.browser_create_host.remote_allocations.summaries(None).is_empty(),
            "a lease nobody recorded holds no allocation"
        );
        assert!(app.browser_create_host.pending.is_empty());
        assert!(
            remote_slots::acquire_slot(&root, &quota, 1).is_ok(),
            "dropping the lease frees its slot for the next allocation"
        );
    }
}
