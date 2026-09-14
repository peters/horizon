//! Azure saved-Shell start on real stores with a fake bound provider: the shared
//! admission, the confirmation snapshot and the drift fences run without the Azure CLI,
//! ARM or SSH.
mod status;

use super::*;
use crate::{
    cloud_run::azure::{AzureDiskSku, AzureProfile, resource_group_name},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_worker_status::{
        ConfiguredRemoteGitStartError, ConfiguredRemotePanelStatusRequest, PreparedRemoteGitStart,
        prepare_configured_remote_git_start,
    },
    remote_workspace::{
        RemoteEnvironmentSummary,
        stop::{ConfiguredStopConfirmationError, configured_azure::Bound},
    },
};

const SUBSCRIPTION: &str = "11111111-1111-4111-8111-111111111111";

fn azure_profile() -> AzureProfile {
    AzureProfile {
        name: "cpu".into(),
        subscription_id: SUBSCRIPTION.into(),
        location: "northeurope".into(),
        vm_size: "Standard_D4s_v3".into(),
        image_pull_identity_id: format!(
            "/subscriptions/{SUBSCRIPTION}/resourceGroups/synthetic/providers/Microsoft.ManagedIdentity/userAssignedIdentities/pull"
        ),
        declared_hourly_cost_micros: 100_000,
        registry_login_server: "synthetic.azurecr.io".into(),
        disk_sku: AzureDiskSku::StandardSsdLrs,
    }
}

struct AzureProvider {
    status: InteractiveWorkerStatus,
    calls: AtomicUsize,
    on_inspect: Option<Box<dyn Fn() + Send + Sync>>,
}

impl InteractiveWorkerProvider for AzureProvider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(Some(self.status.clone()))
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(action) = &self.on_inspect {
            action();
        }
        Ok(Some(self.status.clone()))
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

struct AzureFixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    profile: AzureProfile,
    allocation: StoredRemoteAllocation,
    status: InteractiveWorkerStatus,
}

impl AzureFixture {
    fn new(lifetime: WorkerLifetime, binding: bool, pinned: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).unwrap();
        let identities = RemoteSshIdentityStore::new(&home);
        let profile = azure_profile();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1,"spec":{"workspace_local_id":"workspace","working_directory":"nested","generation":0,
              "panels":[{"panel_local_id":"shell","kind":"shell","command":{"program":"/bin/sh","args":["-c","printf synthetic"]}}],
              "repository":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":"work"},
              "target":{"provider":"azure","profile":"cpu","image":format!("synthetic.azurecr.io/worker@sha256:{}","b".repeat(64)),
                "disk_gib":20,"lifetime":"persistent","max_hourly_cost_micros":200_000}}})).unwrap();
        state.spec.target.lifetime = lifetime;
        let workspace = store.create_remote_workspace(OWNER, &state).unwrap();
        let allocation = store.allocate_remote_runtime(&workspace, i64::MAX).unwrap();
        if binding {
            store
                .record_remote_cpu_profile_binding(&allocation, &profile)
                .expect("immutable binding before any key or provider work");
        }
        let runtime = allocation.workspace().state().runtime.as_ref().unwrap();
        let identity = identities.prepare_new(runtime.workflow_id, runtime.job_id).unwrap();
        let allocation = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
            .unwrap();
        let request = allocation.worker_request().unwrap();
        let group = resource_group_name(request.workflow_id, request.job_id);
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::Azure,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/{group}"),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: match lifetime {
                    WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
                    WorkerLifetime::TimeLimited { seconds } => {
                        InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                            terminate_after: (OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(seconds)))
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap(),
                        })
                    }
                },
            },
            lifecycle: if pinned {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
            ssh: pinned.then(|| InteractiveWorkerSshEndpoint {
                host: "203.0.113.9".into(),
                port: 2222,
                username: "root".into(),
                host_key: identity.public_key().into(),
            }),
        };
        let allocation = store
            .record_remote_worker_recovery(&allocation, Some(&status))
            .expect("synthetic retained observation");
        Self {
            directory,
            store,
            profile,
            allocation,
            status,
        }
    }

    fn config(&self) -> RemoteProviderConfig {
        RemoteProviderConfig {
            azure: vec![self.profile.clone()],
            ..Default::default()
        }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store.load_remote_allocation(OWNER, "workspace").unwrap().unwrap()
    }

    fn provider(&self) -> AzureProvider {
        AzureProvider {
            status: self.status.clone(),
            calls: AtomicUsize::new(0),
            on_inspect: None,
        }
    }

    fn prepare(
        &self,
        config: &RemoteProviderConfig,
        expected: &RemoteEnvironmentSummary,
        owner: &str,
    ) -> Result<PreparedRemoteGitStart, ConfiguredRemoteGitStartError> {
        prepare_configured_remote_git_start(
            &self.store,
            config,
            ConfiguredRemotePanelStatusRequest {
                expected,
                client_session_id: owner,
                panel_id: "shell",
            },
        )
    }
}

/// The confirmation snapshot deliberately has no `Debug` (it carries configuration), so a
/// refusal is unwrapped by hand.
fn refused(result: Result<PreparedRemoteGitStart, ConfiguredRemoteGitStartError>) -> ConfiguredRemoteGitStartError {
    match result {
        Ok(_) => panic!("a refusal was expected"),
        Err(error) => error,
    }
}

#[test]
fn preparation_admits_the_exact_azure_binding_and_refuses_everything_else_before_any_client() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let expected = fixture.allocation.workspace().environment_summary();
    let prepared = fixture.prepare(&fixture.config(), &expected, OWNER).expect("admitted");
    assert_eq!(prepared.panel_id(), "shell");
    assert_eq!(prepared.argv(), ["/bin/sh", "-c", "printf synthetic"]);
    assert_eq!(prepared.work_branch(), "work");
    assert_eq!(prepared.working_directory(), "nested");
    assert_eq!(
        refused(fixture.prepare(&fixture.config(), &expected, "copied-owner")),
        ConfiguredRemoteGitStartError::ClientSessionMismatch
    );
    assert_eq!(
        refused(fixture.prepare(&RemoteProviderConfig::default(), &expected, OWNER)),
        ConfiguredRemoteGitStartError::Configuration(RemoteProviderConfigError::UnconfiguredAzureProfile)
    );
    let mut drifted = fixture.config();
    drifted.azure[0].vm_size = "Standard_D2s_v3".into();
    assert_eq!(
        refused(fixture.prepare(&drifted, &expected, OWNER)),
        ConfiguredRemoteGitStartError::InvalidBinding
    );
    let mut stale = expected.clone();
    stale.revision += 1;
    assert_eq!(
        refused(fixture.prepare(&fixture.config(), &stale, OWNER)),
        ConfiguredRemoteGitStartError::StateChanged
    );
    assert_eq!(fixture.current(), fixture.allocation, "preparation writes nothing");

    let unbound = AzureFixture::new(WorkerLifetime::Persistent, false, true);
    let expected = unbound.allocation.workspace().environment_summary();
    assert_eq!(
        refused(unbound.prepare(&unbound.config(), &expected, OWNER)),
        ConfiguredRemoteGitStartError::InvalidBinding
    );
    let unpinned = AzureFixture::new(WorkerLifetime::Persistent, true, false);
    let expected = unpinned.allocation.workspace().environment_summary();
    assert_eq!(
        refused(unpinned.prepare(&unpinned.config(), &expected, OWNER)),
        ConfiguredRemoteGitStartError::Start(RemoteGitTaskStartError::MissingRetainedWorker)
    );
    // The store refuses a binding for a timed target; the lifetime is refused first.
    let timed = AzureFixture::new(WorkerLifetime::TimeLimited { seconds: 900 }, false, true);
    let expected = timed.allocation.workspace().environment_summary();
    assert_eq!(
        refused(timed.prepare(&timed.config(), &expected, OWNER)),
        ConfiguredRemoteGitStartError::InvalidBinding
    );
}

#[test]
fn saved_stop_stopped_and_start_phases_are_pending_management_before_any_client() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let stopping = fixture
        .store
        .record_remote_stop_phase(
            &fixture.allocation,
            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        )
        .unwrap();
    assert_eq!(
        refused(fixture.prepare(&fixture.config(), &stopping.workspace().environment_summary(), OWNER)),
        ConfiguredRemoteGitStartError::Recovery(RemoteWorkspaceRecoveryError::ManagementPending)
    );
    let stopped = fixture
        .store
        .record_remote_stop_phase(
            &stopping,
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
        )
        .unwrap();
    assert_eq!(
        refused(fixture.prepare(&fixture.config(), &stopped.workspace().environment_summary(), OWNER)),
        ConfiguredRemoteGitStartError::Recovery(RemoteWorkspaceRecoveryError::ManagementPending)
    );
    let starting = fixture
        .store
        .record_remote_start_phase(&stopped, RemoteRuntimePhase::Starting { requested_at_millis: 3 })
        .unwrap();
    assert_eq!(
        refused(fixture.prepare(&fixture.config(), &starting.workspace().environment_summary(), OWNER)),
        ConfiguredRemoteGitStartError::Recovery(RemoteWorkspaceRecoveryError::ManagementPending)
    );
    assert_eq!(fixture.current(), starting, "no phase moved");
}

#[test]
fn dispatch_reaches_the_shared_start_only_with_the_confirmed_binding_intact() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let expected = fixture.allocation.workspace().environment_summary();
    let prepared = fixture.prepare(&fixture.config(), &expected, OWNER).unwrap();
    let started = Cell::new(false);
    let status = super::super::configured::azure_dispatch_with(
        &fixture.store,
        &prepared,
        |_| Ok(fixture.provider()),
        |provider: &Bound<'_, AzureProvider>, allocation, panel| {
            assert_eq!(allocation, &fixture.allocation);
            assert_eq!(panel, "shell");
            let worker = allocation
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.worker.clone());
            provider
                .inspect_worker(worker.as_ref().unwrap())
                .expect("intact binding passes the inspection through");
            started.set(true);
            Ok(RemotePanelStatus::Unavailable)
        },
    )
    .expect("dispatched");
    assert_eq!(status, RemotePanelStatus::Unavailable);
    assert!(started.get());
    assert_eq!(fixture.current(), fixture.allocation);

    // A client failure with an intact snapshot is a binding error, without private detail.
    let error = super::super::configured::azure_dispatch_with::<AzureProvider>(
        &fixture.store,
        &prepared,
        |_| Err(ConfiguredStopConfirmationError::InvalidBinding),
        |_, _, _| panic!("no start without a client"),
    )
    .expect_err("client failure");
    assert_eq!(error, ConfiguredRemoteGitStartError::InvalidBinding);
    assert!(!format!("{error:?} {error}").contains(SUBSCRIPTION));

    // Drift while the client is built outranks the client's own outcome.
    let error = super::super::configured::azure_dispatch_with::<AzureProvider>(
        &fixture.store,
        &prepared,
        |_| {
            let mut next = fixture.allocation.workspace().state().clone();
            next.spec.working_directory = "changed-during-client-construction".into();
            fixture
                .store
                .replace_remote_workspace(fixture.allocation.workspace(), &next)
                .unwrap();
            Ok(fixture.provider())
        },
        |_, _, _| panic!("stale confirmation cannot reach the start"),
    )
    .expect_err("drift at the client");
    assert_eq!(error, ConfiguredRemoteGitStartError::StateChanged);
    // The stale confirmation is refused outright afterwards.
    let error = super::super::configured::azure_dispatch_with::<AzureProvider>(
        &fixture.store,
        &prepared,
        |_| panic!("no client for a stale confirmation"),
        |_, _, _| panic!("no start"),
    )
    .expect_err("stale confirmation");
    assert_eq!(error, ConfiguredRemoteGitStartError::StateChanged);
}

#[test]
fn binding_drift_during_the_inspection_never_sends_the_start() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let expected = fixture.allocation.workspace().environment_summary();
    let prepared = fixture.prepare(&fixture.config(), &expected, OWNER).unwrap();
    let database = fixture.store.path().to_path_buf();
    let mut provider = fixture.provider();
    provider.on_inspect = Some(Box::new(move || drift_binding(&database)));
    let error = super::super::configured::azure_dispatch_with(
        &fixture.store,
        &prepared,
        |_| Ok(provider),
        |provider: &Bound<'_, AzureProvider>, allocation, _| {
            let worker = allocation
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.worker.clone());
            let answer = provider.inspect_worker(worker.as_ref().unwrap());
            assert!(answer.is_err(), "a drifted binding yields no observation");
            // The shared start stops here; nothing is sent after a refused inspection.
            Err(RemoteGitTaskStartError::Recovery(
                RemoteWorkspaceRecoveryError::ProviderUnavailable,
            ))
        },
    )
    .expect_err("binding drift");
    assert_eq!(error, ConfiguredRemoteGitStartError::StateChanged);
}

/// Change the immutable binding row underneath the allocation, as only corruption or a
/// foreign writer could; the store's own triggers are bypassed for the fixture only.
fn drift_binding(path: &std::path::Path) {
    let mut connection = rusqlite::Connection::open(path).expect("fixture database");
    let transaction = connection.transaction().expect("fixture transaction");
    let trigger: String = transaction
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='remote_provider_bindings_no_update'",
            [],
            |row| row.get(0),
        )
        .expect("trigger");
    let digest: String = transaction
        .query_row(
            "SELECT profile_digest FROM remote_provider_bindings WHERE workspace_local_id='workspace'",
            [],
            |row| row.get(0),
        )
        .expect("digest");
    let head = &digest[..digest.len() - 1];
    let flipped = if digest.ends_with('0') {
        format!("{head}1")
    } else {
        format!("{head}0")
    };
    transaction
        .execute_batch("DROP TRIGGER remote_provider_bindings_no_update")
        .expect("fixture only");
    transaction
        .execute(
            "UPDATE remote_provider_bindings SET profile_digest=?1 WHERE workspace_local_id='workspace'",
            [&flipped],
        )
        .expect("binding drift");
    transaction.execute_batch(&trigger).expect("restore exact schema");
    transaction.commit().expect("atomic fixture");
}
