//! Synthetic provider calls exercise the real store/key/coordinator ordering, never ARM or az.
use super::super::{
    ConfiguredWorkspaceSetupObservation as Observation, PreparedRemoteWorkspaceSetup,
    RemoteWorkspaceSetupConsent as Consent, RemoteWorkspaceSetupDraft, check_with, preview_configured_remote_workspace,
    submit_configured_remote_workspace, submit_with,
};
use super::*;
use crate::{
    HorizonHome,
    cloud_run::{
        GitCommitSha, GitSource, WorkerLifetime,
        azure::{AzureDiskSku, resource_group_name},
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
    },
    remote_workspace::RemotePanelCommand,
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const SUBSCRIPTION: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _directory: tempfile::TempDir,
    home: HorizonHome,
    config: RemoteProviderConfig,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        Self {
            home: HorizonHome::from_root(directory.path().join("home")),
            _directory: directory,
            config: RemoteProviderConfig {
                azure: vec![AzureProfile {
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
                }],
                ..Default::default()
            },
        }
    }
    fn profile(&self) -> &AzureProfile {
        &self.config.azure[0]
    }
    fn draft() -> RemoteWorkspaceSetupDraft {
        RemoteWorkspaceSetupDraft {
            target: WorkerTarget {
                provider: CloudProvider::Azure,
                profile: "cpu".into(),
                image: format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                disk_gib: 20,
                lifetime: WorkerLifetime::Persistent,
                max_hourly_cost_micros: Some(200_000),
            },
            repository: GitSource {
                repository: "example/project".into(),
                commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
                branch: Some("work/fixture".into()),
            },
            working_directory: ".".into(),
            command: RemotePanelCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "printf synthetic".into()],
            },
            panel_directory: None,
            retain_until_millis: i64::MAX,
            network_volume: None,
        }
    }
    fn preview(&self) -> PreparedRemoteWorkspaceSetup {
        preview_configured_remote_workspace(&self.home, &self.config, OWNER, Self::draft()).expect("preview")
    }
    fn consent(&self) -> Consent {
        Consent::Azure {
            image: Self::draft().target.image,
            profile: self.profile().clone(),
        }
    }
    fn store(&self) -> CloudWorkflowStore {
        CloudWorkflowStore::open(&self.home).expect("store")
    }
    fn identities(&self) -> RemoteSshIdentityStore {
        RemoteSshIdentityStore::new(&self.home)
    }
    fn load(&self, p: &PreparedRemoteWorkspaceSetup) -> StoredRemoteAllocation {
        self.store()
            .load_remote_allocation(OWNER, &p.locator().workspace_local_id)
            .expect("load")
            .expect("allocation")
    }
    fn start(&self, p: &PreparedRemoteWorkspaceSetup, backend: Backend) -> Result<StoredRemoteAllocation, Error> {
        submit_with(&self.home, &self.config, OWNER, p, &self.consent(), |store, saved| {
            start_with(
                store,
                &self.identities(),
                self.profile(),
                saved,
                p.retain_until_millis(),
                |profile, store| {
                    let allocation = self.load(p);
                    assert!(
                        store
                            .load_remote_cpu_profile_binding(&allocation)
                            .expect("binding")
                            .expect("saved before factory")
                            .matches_profile(profile)
                            .expect("same")
                    );
                    assert!(
                        allocation
                            .workspace()
                            .state()
                            .runtime
                            .as_ref()
                            .expect("runtime")
                            .ssh_public_key
                            .is_none()
                    );
                    assert!(!self.home.root().join("remote-ssh-identities").exists());
                    assert_eq!(backend.counts(), [0; 4]);
                    Ok(backend)
                },
            )
        })
    }
    fn check(
        &self,
        p: &PreparedRemoteWorkspaceSetup,
        config: &RemoteProviderConfig,
        backend: Backend,
    ) -> Result<Observation, Error> {
        check_with(&self.home, config, OWNER, p.locator(), |store, allocation| {
            recover_with(store, &self.identities(), config, allocation, |_, _| Ok(backend))
        })
    }
}

#[derive(Clone)]
struct Backend {
    store: CloudWorkflowStore,
    workspace: String,
    state: Arc<Mutex<BackendState>>,
    lose_response: bool,
}
#[derive(Default)]
struct BackendState {
    // ensure, creation claim, no-handle reconcile, handle inspect
    calls: [usize; 4],
    observed: Option<InteractiveWorkerStatus>,
}
impl Backend {
    fn new(f: &Fixture, p: &PreparedRemoteWorkspaceSetup, lose_response: bool) -> Self {
        Self {
            store: f.store(),
            workspace: p.locator().workspace_local_id.clone(),
            state: Arc::default(),
            lose_response,
        }
    }
    fn counts(&self) -> [usize; 4] {
        self.state.lock().expect("state").calls
    }
}
impl InteractiveWorkerProvider for Backend {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, request: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        let allocation = self
            .store
            .load_remote_allocation(OWNER, &self.workspace)
            .expect("load")
            .expect("allocation");
        assert_eq!(allocation.worker_request().expect("reserved"), *request);
        assert!(
            self.store
                .load_remote_cpu_profile_binding(&allocation)
                .expect("binding")
                .is_some()
        );
        let mut state = self.state.lock().expect("state");
        state.calls[0] += 1;
        let group = resource_group_name(request.workflow_id, request.job_id);
        assert!(
            self.store
                .claim_worker_creation(request.workflow_id, request.job_id, &request.target, &group)
                .expect("fence")
        );
        state.calls[1] += 1;
        let observed = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::Azure,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/{group}"),
                },
                target: request.target.clone(),
                ssh_public_key: request.ssh_public_key.clone(),
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: None,
        };
        state.observed = Some(observed.clone());
        if self.lose_response {
            return Err(std::io::Error::other("synthetic lost response"));
        }
        Ok(InteractiveWorkerEnsure::Created(observed))
    }
    fn reconcile_worker(
        &self,
        request: &InteractiveWorkerRequest,
    ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        let mut state = self.state.lock().expect("state");
        state.calls[2] += 1;
        if let Some(observed) = &state.observed {
            assert_eq!(observed.worker.ssh_public_key, request.ssh_public_key);
        }
        Ok(state.observed.clone())
    }
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        let mut state = self.state.lock().expect("state");
        state.calls[3] += 1;
        assert_eq!(state.observed.as_ref().expect("observation").worker, *worker);
        Ok(state.observed.clone())
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no cleanup")
    }
}

fn change_profile(profile: &mut AzureProfile, field: usize) {
    match field {
        0 => profile.name = "other".into(),
        1 => {
            profile.subscription_id = "22222222-2222-4222-8222-222222222222".into();
            profile.image_pull_identity_id = profile
                .image_pull_identity_id
                .replace(SUBSCRIPTION, &profile.subscription_id);
        }
        2 => profile.location = "westeurope".into(),
        3 => profile.vm_size = "Standard_D8s_v3".into(),
        4 => profile.image_pull_identity_id.push('2'),
        5 => profile.declared_hourly_cost_micros += 1,
        6 => profile.registry_login_server = "other.azurecr.io".into(),
        _ => profile.disk_sku = AzureDiskSku::PremiumLrs,
    }
}

#[test]
fn preview_discloses_complete_profile_without_creating_home() {
    let f = Fixture::new();
    let prepared = f.preview();
    assert_eq!(prepared.azure_profile(), Some(f.profile()));
    assert_eq!(prepared.spec().target, Fixture::draft().target);
    assert!(!f.home.root().exists());
    for mode in 0..5 {
        let mut draft = Fixture::draft();
        match mode {
            0 => draft.target.disk_gib = 4096,
            1 => draft.target.max_hourly_cost_micros = Some(1),
            2 => draft.target.image = format!("other.azurecr.io/worker@sha256:{}", "a".repeat(64)),
            3 => draft.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
            _ => {
                draft.network_volume = Some(crate::cloud_run::runpod::RunPodNetworkVolumeExpectation {
                    volume_id: "synthetic-volume".into(),
                    data_center_id: "synthetic-dc".into(),
                    minimum_size_gb: 10,
                });
            }
        }
        assert!(preview_configured_remote_workspace(&f.home, &f.config, OWNER, draft).is_err());
    }
    assert!(!f.home.root().exists());
}

#[test]
fn every_consent_or_config_field_mismatch_refuses_before_storage_or_dispatch() {
    let f = Fixture::new();
    for field in 0..8 {
        let mut different = f.profile().clone();
        change_profile(&mut different, field);
        let prepared = f.preview();
        let locator = prepared.locator().workspace_local_id.clone();
        let attempt = submit_configured_remote_workspace(
            &f.home,
            &f.config,
            OWNER,
            prepared,
            Consent::Azure {
                image: Fixture::draft().target.image,
                profile: different.clone(),
            },
        );
        assert_eq!(attempt.result, Err(Error::ConsentMismatch));
        assert_eq!(attempt.locator.workspace_local_id, locator);
        let mut config = f.config.clone();
        config.azure[0] = different;
        assert_eq!(
            submit_configured_remote_workspace(&f.home, &config, OWNER, f.preview(), f.consent()).result,
            Err(Error::ContextChanged)
        );
    }
    for consent in [
        Consent::LocalDocker {
            image: Fixture::draft().target.image,
        },
        Consent::Azure {
            image: "wrong-image".into(),
            profile: f.profile().clone(),
        },
    ] {
        assert_eq!(
            submit_configured_remote_workspace(&f.home, &f.config, OWNER, f.preview(), consent).result,
            Err(Error::ConsentMismatch)
        );
    }
    assert!(!f.home.root().exists());
}

#[test]
fn binding_precedes_factory_and_key_and_successful_setup_never_replays() {
    let f = Fixture::new();
    let prepared = f.preview();
    let backend = Backend::new(&f, &prepared, false);
    let result = f.start(&prepared, backend.clone()).expect("setup");
    let request = result.recovery_request().expect("request");
    f.identities()
        .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
        .expect("retained key");
    assert_eq!(backend.counts(), [1, 1, 0, 0]);
    assert_eq!(f.start(&prepared, backend.clone()), Err(Error::SaveConflict));
    assert!(matches!(
        f.check(&prepared, &f.config, backend.clone()),
        Ok(Observation::Observed(_))
    ));
    assert_eq!(backend.counts(), [1, 1, 0, 1]);
}

#[test]
fn lost_creation_response_recovers_exact_no_handle_without_ensure() {
    let f = Fixture::new();
    let prepared = f.preview();
    let backend = Backend::new(&f, &prepared, true);
    assert_eq!(f.start(&prepared, backend.clone()), Err(Error::SetupUnconfirmed));
    let before = f.load(&prepared);
    assert!(
        before
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_none()
    );
    let request = before.recovery_request().expect("request");
    let binding = f.store().load_remote_cpu_profile_binding(&before).expect("binding");
    assert!(matches!(
        f.check(&prepared, &f.config, backend.clone()),
        Ok(Observation::Observed(_))
    ));
    let after = f.load(&prepared);
    assert_eq!(after.recovery_request().expect("request"), request);
    assert_eq!(
        f.store().load_remote_cpu_profile_binding(&after).expect("binding"),
        binding
    );
    assert_eq!(backend.counts(), [1, 1, 1, 0]);
    assert!(matches!(
        f.check(&prepared, &f.config, backend.clone()),
        Ok(Observation::Observed(_))
    ));
    assert_eq!(backend.counts(), [1, 1, 1, 1]);
}

#[test]
fn interrupted_before_key_never_repairs_and_missing_binding_never_backfills() {
    for record_binding in [false, true] {
        let f = Fixture::new();
        let p = f.preview();
        let store = f.store();
        let saved = store.create_remote_workspace(OWNER, &p.state).expect("saved");
        let allocation = if record_binding {
            assert_eq!(
                start_with::<Backend>(&store, &f.identities(), f.profile(), &saved, i64::MAX, |_, _| Err(
                    Error::SetupUnconfirmed
                )),
                Err(Error::SetupUnconfirmed)
            );
            f.load(&p)
        } else {
            store.allocate_remote_runtime(&saved, i64::MAX).expect("allocate")
        };
        assert!(matches!(
            check_with(&f.home, &f.config, OWNER, p.locator(), |_, _| panic!(
                "must not dispatch"
            )),
            Ok(Observation::Interrupted(_))
        ));
        assert_eq!(
            store
                .load_remote_cpu_profile_binding(&allocation)
                .expect("binding")
                .is_some(),
            record_binding
        );
        assert_eq!(f.load(&p), allocation);
        assert!(!f.home.root().join("remote-ssh-identities").exists());
        assert_eq!(
            recover_with::<Backend>(&store, &f.identities(), &f.config, &allocation, |_, _| Err(
                Error::SetupUnconfirmed
            )),
            Err(Error::SetupUnconfirmed)
        );
    }
}

#[test]
fn changed_profiles_refuse_no_handle_recovery_before_factory_or_key_access() {
    let f = Fixture::new();
    let prepared = f.preview();
    let backend = Backend::new(&f, &prepared, true);
    assert!(f.start(&prepared, backend.clone()).is_err());
    let allocation = f.load(&prepared);
    for field in 0..8 {
        let mut config = f.config.clone();
        change_profile(&mut config.azure[0], field);
        assert!(
            check_with(&f.home, &config, OWNER, prepared.locator(), |_, _| panic!(
                "must not dispatch"
            ))
            .is_err()
        );
        assert!(
            recover_with::<Backend>(&f.store(), &f.identities(), &config, &allocation, |_, _| panic!(
                "must not construct provider"
            ))
            .is_err()
        );
    }
    assert_eq!(backend.counts(), [1, 1, 0, 0]);
    assert_eq!(f.load(&prepared), allocation);
}

#[test]
fn stale_allocation_refuses_before_recovery_factory() {
    let f = Fixture::new();
    let p = f.preview();
    let store = f.store();
    let saved = store.create_remote_workspace(OWNER, &p.state).expect("saved");
    let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocate");
    store
        .record_remote_cpu_profile_binding(&allocation, f.profile())
        .expect("binding");
    super::super::super::prepare_identity(&store, &f.identities(), &allocation).expect("reserve");
    assert_eq!(
        recover_with::<Backend>(&store, &f.identities(), &f.config, &allocation, |_, _| panic!(
            "stale snapshot must not construct provider"
        )),
        Err(Error::StorageUnavailable)
    );
}
