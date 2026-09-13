use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, WorkerLifetime, interactive_worker::*},
    remote_workspace::RemoteWorkspaceState,
};
use std::os::unix::fs::PermissionsExt;

mod azure;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH";
const SUBSCRIPTION: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    config: RemoteProviderConfig,
}
impl Fixture {
    fn new(provider: CloudProvider, network: bool) -> Self {
        Self::with_worker(provider, network, WorkerLifetime::Persistent, true)
    }
    fn with_worker(provider: CloudProvider, network: bool, lifetime: WorkerLifetime, pinned: bool) -> Self {
        Self::configured(provider, network, lifetime, pinned, Some("work/one"))
    }
    fn configured(
        provider: CloudProvider,
        network: bool,
        lifetime: WorkerLifetime,
        pinned: bool,
        branch: Option<&str>,
    ) -> Self {
        Self::build(provider, network, lifetime, pinned, branch, false)
    }
    fn build(
        provider: CloudProvider,
        network: bool,
        lifetime: WorkerLifetime,
        pinned: bool,
        branch: Option<&str>,
        private_identity: bool,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).unwrap();
        let config: RemoteProviderConfig = serde_json::from_value(serde_json::json!({
            "local_docker":[{"name":"development","docker_host":"unix:///tmp/synthetic-docker.sock"}],
            "runpod":[{"name":"development","gpu_type_ids":["synthetic-gpu"],"gpu_count":1,"ports":["22/tcp"],"volume_gib":0,"data_center_id":"EU-RO-1"}],
            "azure":[{"name":"development","subscription_id":SUBSCRIPTION,"location":"northeurope",
                "vm_size":"Standard_D4s_v3","declared_hourly_cost_micros":100_000,
                "image_pull_identity_id":format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/synthetic/providers/Microsoft.ManagedIdentity/userAssignedIdentities/pull"),
                "registry_login_server":"synthetic.azurecr.io","disk_sku":"StandardSSD_LRS"}]
        })).unwrap();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({"version":1,"spec":{
            "workspace_local_id":"workspace","working_directory":"nested","generation":0,
            "repository":{"repository":"fixture/project","commit":"a".repeat(40),"branch":"work/one"},
            "target":{"provider":provider,"profile":"development","image":format!("fixture/worker@sha256:{}","b".repeat(64)),"disk_gib":20,"lifetime":"persistent"},
            "panels":[{"panel_local_id":"shell","kind":"shell","command":{"program":"/bin/sh","args":["-c","printf synthetic"]}}]
        }})).unwrap();
        state.spec.target.lifetime = lifetime;
        state.spec.repository.branch = branch.map(str::to_owned);
        if provider == CloudProvider::Azure {
            state.spec.target.image = format!("synthetic.azurecr.io/worker@sha256:{}", "b".repeat(64));
        }
        let workspace = store.create_remote_workspace(OWNER, &state).unwrap();
        let allocation = store.allocate_remote_runtime(&workspace, i64::MAX).unwrap();
        if provider == CloudProvider::Azure {
            store
                .record_remote_cpu_profile_binding(&allocation, &config.azure[0])
                .unwrap();
        }
        if network {
            store
                .record_remote_network_volume_selection(
                    &allocation,
                    &RunPodNetworkVolumeExpectation {
                        volume_id: "fixture-volume".into(),
                        data_center_id: "EU-RO-1".into(),
                        minimum_size_gb: 10,
                    },
                )
                .unwrap();
        }
        let runtime = allocation.workspace().state().runtime.as_ref().unwrap();
        let key = if private_identity {
            RemoteSshIdentityStore::new(&home)
                .prepare_new(runtime.workflow_id, runtime.job_id)
                .unwrap()
                .public_key()
                .to_owned()
        } else {
            KEY.to_owned()
        };
        let allocation = store.reserve_remote_worker_request(&allocation, &key).unwrap();
        let request = allocation.worker_request().unwrap();
        let resource = if provider == CloudProvider::Azure {
            let group = crate::cloud_run::azure::resource_group_name(request.workflow_id, request.job_id);
            format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/{group}")
        } else {
            "fixture-worker".into()
        };
        store
            .record_remote_worker_recovery(
                &allocation,
                Some(&InteractiveWorkerStatus {
                    worker: InteractiveWorker {
                        identity: InteractiveWorkerIdentity {
                            provider,
                            workflow_id: request.workflow_id,
                            job_id: request.job_id,
                            resource_id: resource,
                        },
                        target: request.target,
                        ssh_public_key: request.ssh_public_key,
                        lifetime: match lifetime {
                            WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
                            WorkerLifetime::TimeLimited { .. } => {
                                InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                                    terminate_after: "2099-01-01T00:00:00Z".into(),
                                })
                            }
                        },
                    },
                    lifecycle: InteractiveWorkerLifecycle::Provisioning,
                    ssh: pinned.then(|| InteractiveWorkerSshEndpoint {
                        host: "127.0.0.1".into(),
                        port: 2222,
                        username: "root".into(),
                        host_key: KEY.into(),
                    }),
                }),
            )
            .unwrap();
        Self {
            directory,
            store,
            config,
        }
    }
    fn current(&self) -> StoredRemoteAllocation {
        self.store.load_remote_allocation(OWNER, "workspace").unwrap().unwrap()
    }
    fn summary(&self) -> RemoteEnvironmentSummary {
        self.current().workspace().environment_summary()
    }
    fn edit(&self, change: impl FnOnce(&mut RemoteWorkspaceState)) {
        let current = self.current();
        let mut state = current.workspace().state().clone();
        change(&mut state);
        self.store
            .replace_remote_workspace(current.workspace(), &state)
            .unwrap();
    }
}
fn request(expected: &RemoteEnvironmentSummary) -> ConfiguredRemoteGitSetupRequest<'_> {
    ConfiguredRemoteGitSetupRequest {
        expected,
        client_session_id: OWNER,
    }
}
fn preview(f: &Fixture, credentials: RemoteGitCredentialMode) -> PreparedRemoteGitSetup {
    prepare(&f.store, &f.config, request(&f.summary()), credentials).unwrap()
}
fn identities(f: &Fixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(f.directory.path().join("missing-keys")))
}

#[test]
fn preview_binds_saved_source_destination_and_mode_without_secrets() {
    for provider in [CloudProvider::LocalDocker, CloudProvider::RunPod] {
        let f = Fixture::new(provider, provider == CloudProvider::RunPod);
        let before = f.current();
        for mode in [
            RemoteGitCredentialMode::UseInstalled,
            RemoteGitCredentialMode::InstallFirst,
        ] {
            let prepared = preview(&f, mode);
            assert_eq!(prepared.repository(), "fixture/project");
            assert_eq!(prepared.commit(), "a".repeat(40));
            assert_eq!(prepared.work_branch(), "work/one");
            assert_eq!(prepared.credential_mode(), mode);
            assert_eq!(prepared.environment(), &f.summary());
            assert_eq!(
                prepared.environment().worker_identity.as_ref().unwrap().resource_id,
                "fixture-worker"
            );
        }
        assert_eq!(before, f.current());
        assert!(!f.directory.path().join("missing-keys").exists());
    }
}

#[test]
fn unsupported_lifetime_missing_pin_and_foreign_owner_fail_before_dispatch() {
    for (lifetime, pinned) in [
        (WorkerLifetime::TimeLimited { seconds: 300 }, true),
        (WorkerLifetime::Persistent, false),
    ] {
        let f = Fixture::with_worker(CloudProvider::RunPod, false, lifetime, pinned);
        assert!(
            prepare(
                &f.store,
                &f.config,
                request(&f.summary()),
                RemoteGitCredentialMode::UseInstalled
            )
            .is_err()
        );
    }
    let f = Fixture::new(CloudProvider::RunPod, true);
    let summary = f.summary();
    let foreign = ConfiguredRemoteGitSetupRequest {
        expected: &summary,
        client_session_id: "foreign",
    };
    assert!(matches!(
        prepare(&f.store, &f.config, foreign, RemoteGitCredentialMode::UseInstalled),
        Err(ConfiguredRemoteGitSetupError::ClientSessionMismatch)
    ));
    let mut config = f.config.clone();
    config.runpod.clear();
    assert!(
        prepare(
            &f.store,
            &config,
            request(&summary),
            RemoteGitCredentialMode::UseInstalled
        )
        .is_err()
    );
}

#[test]
fn full_snapshot_not_summary_authorizes_confirmation() {
    let f = Fixture::new(CloudProvider::RunPod, true);
    let mut prepared = preview(&f, RemoteGitCredentialMode::UseInstalled);
    f.edit(|state| {
        state.spec.panels[0]
            .command
            .as_mut()
            .unwrap()
            .args
            .push("changed".into());
    });
    prepared.expected = f.summary();
    assert!(matches!(
        check_snapshot(&f.store, &f.config, request(&f.summary()), &prepared),
        Err(ConfiguredRemoteGitSetupError::StateChanged)
    ));
    let mut prepared = preview(&f, RemoteGitCredentialMode::UseInstalled);
    prepared.branch = "different".into();
    assert!(check_snapshot(&f.store, &f.config, request(&f.summary()), &prepared).is_err());
    let mut prepared = preview(&f, RemoteGitCredentialMode::UseInstalled);
    prepared.selection.as_mut().unwrap().volume_id = "different-volume".into();
    assert!(check_snapshot(&f.store, &f.config, request(&f.summary()), &prepared).is_err());
    let mut prepared = preview(&f, RemoteGitCredentialMode::UseInstalled);
    prepared.config.runpod.clear();
    assert!(check_snapshot(&f.store, &f.config, request(&f.summary()), &prepared).is_err());
}

#[test]
fn absent_or_invalid_saved_branch_is_not_invented() {
    for branch in [None, Some("HEAD")] {
        let f = Fixture::configured(
            CloudProvider::LocalDocker,
            false,
            WorkerLifetime::Persistent,
            true,
            branch,
        );
        assert!(
            prepare(
                &f.store,
                &f.config,
                request(&f.summary()),
                RemoteGitCredentialMode::UseInstalled
            )
            .is_err()
        );
    }
}

#[test]
fn explicit_secret_consent_must_match_before_key_or_provider_access() {
    let f = Fixture::new(CloudProvider::RunPod, true);
    let token = RepositoryPat::new("synthetic_repository_pat").unwrap();
    for (mode, token) in [
        (RemoteGitCredentialMode::UseInstalled, Some(&token)),
        (RemoteGitCredentialMode::InstallFirst, None),
    ] {
        let prepared = preview(&f, mode);
        assert!(matches!(
            submit_configured_remote_git_setup(
                &f.store,
                &identities(&f),
                &f.config,
                request(&f.summary()),
                prepared,
                token
            ),
            Err(ConfiguredRemoteGitSetupError::CredentialConsentMismatch)
        ));
    }
    assert!(!f.directory.path().join("missing-keys").exists());
}

#[test]
fn missing_private_identity_prevents_both_submit_and_inspect() {
    let f = Fixture::new(CloudProvider::RunPod, true);
    assert!(matches!(
        submit_configured_remote_git_setup(
            &f.store,
            &identities(&f),
            &f.config,
            request(&f.summary()),
            preview(&f, RemoteGitCredentialMode::UseInstalled),
            None
        ),
        Err(ConfiguredRemoteGitSetupError::Recovery(_))
    ));
    assert!(matches!(
        inspect_configured_remote_git_setup(&f.store, &identities(&f), &f.config, request(&f.summary())),
        Err(ConfiguredRemoteGitSetupError::Recovery(_))
    ));
    assert!(!f.directory.path().join("missing-keys").exists());
}

#[test]
fn installed_and_present_continue_once_but_no_token_mode_skips_install() {
    use std::cell::Cell;
    let token = RepositoryPat::new("synthetic_repository_pat").unwrap();
    for installation in [
        RemoteCredentialInstallation::Installed,
        RemoteCredentialInstallation::Present,
    ] {
        let calls = Cell::new(0);
        let result = submit_with(
            || Ok(()),
            Some(&token),
            |_| {
                assert_eq!(calls.replace(1), 0);
                Ok(installation)
            },
            || {
                assert_eq!(calls.replace(2), 1);
                Ok(RemoteGitSubmission::Submitted)
            },
        )
        .unwrap();
        assert_eq!(result.credential, Some(installation));
        assert_eq!(result.submission, RemoteGitSubmission::Submitted);
        assert_eq!(calls.get(), 2);
    }
    let result = submit_with(
        || Ok(()),
        None,
        |_| panic!("no implicit installation"),
        || Ok(RemoteGitSubmission::Unknown),
    )
    .unwrap();
    assert_eq!(result.credential, None);
    assert_eq!(result.submission, RemoteGitSubmission::Unknown);
}

#[test]
fn refused_or_uncertain_delivery_never_submits_git() {
    let token = RepositoryPat::new("synthetic_repository_pat").unwrap();
    for error in [
        ConfiguredRemoteGitSetupError::Credential(RemoteCredentialDeliveryError::InvalidToken),
        ConfiguredRemoteGitSetupError::OutcomeUnknown,
    ] {
        let result = submit_with(|| Ok(()), Some(&token), |_| Err(error), || panic!("must not submit"));
        assert!(result.is_err());
    }
}

#[test]
fn each_mutation_boundary_checks_drift_even_when_exchange_errors() {
    use std::cell::Cell;
    let token = RepositoryPat::new("synthetic_repository_pat").unwrap();
    for failed_check in 1..=4 {
        let checks = Cell::new(0);
        let installs = Cell::new(0);
        let submissions = Cell::new(0);
        let result = submit_with(
            || {
                let count = checks.get() + 1;
                checks.set(count);
                if count == failed_check {
                    Err(ConfiguredRemoteGitSetupError::StateChanged)
                } else {
                    Ok(())
                }
            },
            Some(&token),
            |_| {
                installs.set(installs.get() + 1);
                Ok(RemoteCredentialInstallation::Installed)
            },
            || {
                submissions.set(submissions.get() + 1);
                Err(ConfiguredRemoteGitSetupError::Git(RemoteGitSetupError::Rejected))
            },
        );
        assert_eq!(
            result,
            Err(if failed_check == 1 {
                ConfiguredRemoteGitSetupError::StateChanged
            } else {
                ConfiguredRemoteGitSetupError::OutcomeUnknown
            })
        );
        assert_eq!(installs.get(), u32::from(failed_check > 1));
        assert_eq!(submissions.get(), u32::from(failed_check > 3));
    }
    let changed = Cell::new(false);
    let result = submit_with(
        || {
            if changed.get() {
                Err(ConfiguredRemoteGitSetupError::StateChanged)
            } else {
                Ok(())
            }
        },
        Some(&token),
        |_| {
            changed.set(true);
            Err(ConfiguredRemoteGitSetupError::Credential(
                RemoteCredentialDeliveryError::InvalidToken,
            ))
        },
        || panic!("must not submit after uncertain delivery"),
    );
    assert_eq!(result, Err(ConfiguredRemoteGitSetupError::OutcomeUnknown));
}

#[test]
fn observation_variants_are_not_promoted_to_readiness_or_resubmitted() {
    use super::super::{RemoteGitReason, RemoteGitState};
    for observation in [
        RemoteGitObservation {
            state: RemoteGitState::Absent,
            reason: None,
        },
        RemoteGitObservation {
            state: RemoteGitState::ClaimedUnknown,
            reason: None,
        },
        RemoteGitObservation {
            state: RemoteGitState::Complete,
            reason: None,
        },
        RemoteGitObservation {
            state: RemoteGitState::Complete,
            reason: Some(RemoteGitReason::Conflict),
        },
    ] {
        let result = submit_with(
            || Ok(()),
            None,
            |_| panic!("no credentials"),
            || Ok(RemoteGitSubmission::Observed(observation)),
        )
        .unwrap();
        assert_eq!(result.submission, RemoteGitSubmission::Observed(observation));
    }
}
