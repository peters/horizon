use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, WorkerLifetime, interactive_worker::*},
    remote_workspace::RemoteWorkspaceState,
};
use std::os::unix::fs::PermissionsExt;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH";

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
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = CloudWorkflowStore::open(&HorizonHome::from_root(directory.path().join("home"))).unwrap();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({"version":1,"spec":{
            "workspace_local_id":"workspace","working_directory":"nested","generation":0,
            "repository":{"repository":"fixture/project","commit":"a".repeat(40),"branch":"work/one"},
            "target":{"provider":provider,"profile":"development","image":format!("fixture/worker@sha256:{}","b".repeat(64)),"disk_gib":20,"lifetime":"persistent"},
            "panels":[{"panel_local_id":"shell","kind":"shell","command":{"program":"/bin/sh","args":["-c","printf synthetic"]}}]
        }})).unwrap();
        state.spec.target.lifetime = lifetime;
        let workspace = store.create_remote_workspace(OWNER, &state).unwrap();
        let allocation = store.allocate_remote_runtime(&workspace, i64::MAX).unwrap();
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
        let allocation = store.reserve_remote_worker_request(&allocation, KEY).unwrap();
        let request = allocation.worker_request().unwrap();
        store
            .record_remote_worker_recovery(
                &allocation,
                Some(&InteractiveWorkerStatus {
                    worker: InteractiveWorker {
                        identity: InteractiveWorkerIdentity {
                            provider,
                            workflow_id: request.workflow_id,
                            job_id: request.job_id,
                            resource_id: "fixture-worker".into(),
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
        let config = serde_json::from_value(serde_json::json!({
            "local_docker":[{"name":"development","docker_host":"unix:///tmp/synthetic-docker.sock"}],
            "runpod":[{"name":"development","gpu_type_ids":["synthetic-gpu"],"gpu_count":1,"ports":["22/tcp"],"volume_gib":0,"data_center_id":"EU-RO-1"}]
        })).unwrap();
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
fn request(expected: &RemoteEnvironmentSummary) -> ConfiguredRemotePanelStatusRequest<'_> {
    ConfiguredRemotePanelStatusRequest {
        expected,
        client_session_id: OWNER,
        panel_id: "shell",
    }
}

#[test]
fn preview_is_saved_only_without_private_keys_or_provider_io() {
    for (provider, network) in [
        (CloudProvider::LocalDocker, false),
        (CloudProvider::RunPod, false),
        (CloudProvider::RunPod, true),
    ] {
        let f = Fixture::new(provider, network);
        let before = std::fs::read(f.store.path()).unwrap();
        let prepared = prepare_configured_remote_git_start(&f.store, &f.config, request(&f.summary())).unwrap();
        assert_eq!(
            (prepared.repository(), prepared.commit(), prepared.work_branch()),
            ("fixture/project", "a".repeat(40).as_str(), "work/one")
        );
        assert_eq!((prepared.panel_id(), prepared.working_directory()), ("shell", "nested"));
        assert_eq!(prepared.argv(), ["/bin/sh", "-c", "printf synthetic"]);
        assert_eq!(std::fs::read(f.store.path()).unwrap(), before);
        let result = confirmed(&f.store, &f.config, request(&f.summary()), &prepared, || {
            Ok(RemotePanelStatus::Exited {
                pid: 12,
                exit_status: Some(7),
            })
        });
        assert_eq!(
            result,
            Ok(RemotePanelStatus::Exited {
                pid: 12,
                exit_status: Some(7)
            })
        );
    }
}

#[test]
fn changes_to_saved_command_config_or_selection_do_not_dispatch() {
    let f = Fixture::new(CloudProvider::LocalDocker, false);
    let summary = f.summary();
    let mut prepared = prepare(&f.store, &f.config, request(&summary)).unwrap();
    let mut config = f.config.clone();
    config.local_docker[0].docker_host = "unix:///tmp/changed.sock".into();
    assert_eq!(
        confirmed(&f.store, &config, request(&summary), &prepared, || panic!(
            "no dispatch"
        )),
        Err(ConfiguredRemoteGitStartError::StateChanged)
    );
    let mut changed = request(&summary);
    changed.client_session_id = "other";
    assert_eq!(
        confirmed(&f.store, &f.config, changed, &prepared, || panic!("no dispatch")),
        Err(ConfiguredRemoteGitStartError::ClientSessionMismatch)
    );
    f.edit(|state| {
        state.spec.panels[0]
            .command
            .as_mut()
            .unwrap()
            .args
            .push("changed".into());
    });
    // Even a matching projected summary cannot substitute for the full saved intent.
    prepared.expected = f.summary();
    assert_eq!(
        confirmed(&f.store, &f.config, request(&f.summary()), &prepared, || panic!(
            "no dispatch"
        )),
        Err(ConfiguredRemoteGitStartError::StateChanged)
    );
    let f = Fixture::new(CloudProvider::RunPod, true);
    let mut prepared = prepare(&f.store, &f.config, request(&f.summary())).unwrap();
    // Simulate a confirmation captured before the separate selection existed.
    prepared.selection = None;
    assert_eq!(
        confirmed(&f.store, &f.config, request(&f.summary()), &prepared, || panic!(
            "no dispatch"
        )),
        Err(ConfiguredRemoteGitStartError::StateChanged)
    );
}

#[test]
fn post_dispatch_drift_is_unknown_even_when_exchange_returns_an_error() {
    for failed in [false, true] {
        let f = Fixture::new(CloudProvider::RunPod, true);
        let summary = f.summary();
        let prepared = prepare(&f.store, &f.config, request(&summary)).unwrap();
        let result = confirmed(&f.store, &f.config, request(&summary), &prepared, || {
            f.edit(|state| state.spec.working_directory = "changed".into());
            if failed {
                Err(ConfiguredRemoteGitStartError::InvalidBinding)
            } else {
                Ok(RemotePanelStatus::Running { pid: 12 })
            }
        });
        assert_eq!(result, Err(ConfiguredRemoteGitStartError::OutcomeUnknown));
    }
    assert_eq!(
        ConfiguredRemoteGitStartError::from(RemoteGitTaskStartError::OutcomeUnknown),
        ConfiguredRemoteGitStartError::OutcomeUnknown
    );
}

#[test]
fn absent_command_pin_or_matching_profile_cannot_be_confirmed() {
    let limited = Fixture::with_worker(
        CloudProvider::LocalDocker,
        false,
        WorkerLifetime::TimeLimited { seconds: 60 },
        true,
    );
    assert!(matches!(
        prepare(&limited.store, &limited.config, request(&limited.summary())),
        Err(ConfiguredRemoteGitStartError::InvalidBinding)
    ));
    for mode in 0..4 {
        let mut f = Fixture::with_worker(CloudProvider::RunPod, true, WorkerLifetime::Persistent, mode != 1);
        match mode {
            0 => f.edit(|state| state.spec.panels[0].command = None),
            1 => (),
            2 => f.config.runpod[0].data_center_id = Some("EU-FR-1".into()),
            _ => f.config.runpod.clear(),
        }
        assert!(prepare(&f.store, &f.config, request(&f.summary())).is_err());
    }
}

#[test]
fn private_key_is_checked_only_during_confirmed_execution() {
    let f = Fixture::new(CloudProvider::RunPod, true);
    let summary = f.summary();
    let prepared = prepare(&f.store, &f.config, request(&summary)).unwrap();
    let missing = RemoteSshIdentityStore::new(&HorizonHome::from_root(f.directory.path().join("missing")));
    assert!(matches!(
        start_configured_remote_git_shell(&f.store, &missing, &f.config, request(&summary), prepared),
        Err(ConfiguredRemoteGitStartError::Recovery(_))
    ));
}
