use super::super::configured::runpod_with;
use super::*;
use crate::{
    cloud_run::runpod::{RunPodApiKey, RunPodError, RunPodNetworkVolumeExpectation},
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::{RemoteEnvironmentSummary, RemoteRuntimePhase},
    remote_workspace_recovery::{RemoteWorkspaceRecoveryError, inspect_remote_allocation},
};

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        runpod: vec![
            serde_json::from_value(serde_json::json!({
                "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
                "ports":["22/tcp"], "volume_gib":0, "data_center_id":"EU-RO-1"
            }))
            .expect("profile"),
        ],
        ..Default::default()
    }
}

fn fixture(network: bool) -> Fixture {
    let selection = RunPodNetworkVolumeExpectation {
        volume_id: "synthetic_volume".into(),
        data_center_id: "EU-RO-1".into(),
        minimum_size_gb: 10,
    };
    Fixture::with_provider(
        InteractiveWorkerLifecycle::Ready,
        CloudProvider::RunPod,
        network.then_some(&selection),
    )
}

fn identities(fixture: &Fixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")))
}

fn request(expected: &RemoteEnvironmentSummary) -> ConfiguredRemotePanelStatusRequest<'_> {
    ConfiguredRemotePanelStatusRequest {
        expected,
        client_session_id: OWNER,
        panel_id: "terminal",
    }
}

fn key() -> Result<RunPodApiKey, ConfiguredRemotePanelStatusError> {
    RunPodApiKey::new("synthetic-private-credential")
        .map_err(|_| ConfiguredRemotePanelStatusError::RunPodCredentialUnavailable)
}

struct ExactObserver<'a> {
    status: &'a InteractiveWorkerStatus,
    calls: std::sync::atomic::AtomicUsize,
}

impl InteractiveWorkerProvider for ExactObserver<'_> {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::RunPod
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("inspection cannot create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("inspection requires the retained exact worker")
    }
    fn inspect_worker(&self, worker: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        assert_eq!(worker, &self.status.worker);
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(self.status.clone()))
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("inspection cannot delete")
    }
}

#[test]
fn retained_and_selected_status_is_read_only_even_after_setup_retention_expires() {
    for network in [false, true] {
        let fixture = fixture(network);
        let current = fixture.current();
        let mut workflow = current.workflow().workflow().clone();
        workflow.created_at_millis = 1000;
        workflow.updated_at_millis = 1000;
        workflow.retain_until_millis = 2000;
        rusqlite::Connection::open(fixture.store.path())
            .expect("database")
            .execute(
                "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
             retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
                rusqlite::params![
                    serde_json::to_vec(&workflow).expect("snapshot"),
                    workflow.id.to_string()
                ],
            )
            .expect("expired setup fixture");
        let before = fixture.current();
        assert!(
            before.workspace().state().spec.panels[0].command.is_none(),
            "status is not launch admission"
        );
        let database = std::fs::read(fixture.store.path()).expect("database bytes");
        let private_key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key bytes");
        let selection = fixture
            .store
            .load_remote_network_volume_selection(&before)
            .expect("selection");
        let observer = ExactObserver {
            status: fixture.recovered.observation().expect("observation"),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        for response in [
            RUNNING,
            br#"{"state":"exited","panel":"terminal","pid":123,"exit_status":7}"#,
            br#"{"state":"exited","panel":"terminal","pid":123,"exit_status":null}"#,
            br#"{"state":"unavailable","panel":"terminal"}"#,
        ] {
            let result = runpod_with(
                &fixture.store,
                &identities(&fixture),
                &config(),
                request(&before.workspace().environment_summary()),
                key,
                |provider, allocation| {
                    assert_eq!(allocation, &before);
                    assert_eq!(provider.provider(), CloudProvider::RunPod);
                    if network {
                        let mut wrong = observer.status.worker.clone();
                        wrong.target.disk_gib += 1;
                        assert_eq!(provider.inspect_worker(&wrong), Err(RunPodError::InvalidTarget));
                    }
                    let recovered = inspect_remote_allocation(&identities(&fixture), &observer, allocation)?;
                    Ok(super::super::inspect_with(
                        &fixture.store,
                        &recovered,
                        "terminal",
                        Inspection::Status,
                        |_, _, input| {
                            let input: serde_json::Value = serde_json::from_slice(input).expect("status input");
                            assert_eq!(input["operation"], "status");
                            assert_eq!(input["panel"], "terminal");
                            Ok(response.to_vec())
                        },
                    )?)
                },
            )
            .expect("read-only observation");
            assert_eq!(result.panel_id, "terminal");
            assert_eq!(result.status, protocol::response(response, "terminal").expect("status"));
            assert!(result.observed_at_rfc3339().is_some());
            assert_eq!(fixture.current(), before);
        }
        assert_eq!(observer.calls.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(std::fs::read(fixture.store.path()).expect("database bytes"), database);
        assert_eq!(
            std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
            private_key
        );
        assert_eq!(
            fixture
                .store
                .load_remote_network_volume_selection(&before)
                .expect("selection"),
            selection
        );
        assert!(before.workflow().workflow().retain_until_millis < 3000);
    }
}

#[test]
fn local_admission_rejects_before_credentials_provider_or_ssh() {
    let fixture = fixture(true);
    let before = fixture.current();
    for fault in 0..9 {
        let mut expected = before.workspace().environment_summary();
        let mut config = config();
        let mut owner = OWNER;
        let mut panel = "terminal";
        let error = match fault {
            0 => {
                owner = "foreign-owner";
                ConfiguredRemotePanelStatusError::ClientSessionMismatch
            }
            1 => {
                expected.provider = CloudProvider::Azure;
                ConfiguredRemotePanelStatusError::UnsupportedProvider
            }
            2 => {
                config.runpod.clear();
                RemoteProviderConfigError::UnconfiguredRunPodProfile.into()
            }
            3 => {
                config.runpod[0].name = "Development".into();
                RemoteProviderConfigError::UnconfiguredRunPodProfile.into()
            }
            4 => {
                expected.revision += 1;
                RemoteWorkspaceRecoveryError::StateChanged.into()
            }
            5 => {
                expected.panel_count += 1;
                RemoteWorkspaceRecoveryError::StateChanged.into()
            }
            6 => {
                panel = "unknown;start";
                RemotePanelStatusError::UnknownPanel.into()
            }
            7 => {
                config.runpod[0].gpu_count = 0;
                ConfiguredRemotePanelStatusError::InvalidRunPodBinding
            }
            _ => {
                config.runpod[0].data_center_id = Some("US-CA-1".into());
                ConfiguredRemotePanelStatusError::InvalidRunPodBinding
            }
        };
        assert_eq!(
            runpod_with(
                &fixture.store,
                &identities(&fixture),
                &config,
                ConfiguredRemotePanelStatusRequest {
                    expected: &expected,
                    client_session_id: owner,
                    panel_id: panel
                },
                || panic!("no credentials"),
                |_, _| panic!("no inspection")
            ),
            Err(error)
        );
    }
    assert_eq!(fixture.current(), before);
}

#[test]
fn missing_identity_or_worker_and_pending_management_cannot_repair_or_query() {
    let fixture = fixture(false);
    let before = fixture.current();
    let path = fixture.recovered.identity().private_key_path();
    let retained = path.with_extension("retained-fixture");
    std::fs::rename(path, &retained).expect("hide fixture key");
    assert!(matches!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&before.workspace().environment_summary()),
            || panic!("no credentials"),
            |_, _| panic!("no query")
        ),
        Err(ConfiguredRemotePanelStatusError::Recovery(
            RemoteWorkspaceRecoveryError::Identity(_)
        ))
    ));
    assert!(!path.exists() && retained.is_file());
    let stopped = fixture
        .store
        .record_remote_stop_phase(&before, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
        .expect("stop intent");
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&stopped.workspace().environment_summary()),
            || panic!("no credentials"),
            |_, _| panic!("no query")
        ),
        Err(RemoteWorkspaceRecoveryError::ManagementPending.into())
    );
    let mut state = before.workspace().state().clone();
    state.spec.workspace_local_id = "unobserved".into();
    state.spec.generation = 0;
    state.runtime = None;
    let dormant = fixture.store.create_remote_workspace(OWNER, &state).expect("dormant");
    let unobserved = fixture
        .store
        .allocate_remote_runtime(&dormant, i64::MAX)
        .expect("allocation");
    let unobserved = fixture
        .store
        .reserve_remote_worker_request(&unobserved, fixture.recovered.identity().public_key())
        .expect("request");
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&unobserved.workspace().environment_summary()),
            || panic!("no credentials"),
            |_, _| panic!("no query")
        ),
        Err(RemotePanelStatusError::WorkerUnavailable.into())
    );
    assert_eq!(fixture.current(), stopped);
    let mut status = fixture.recovered.observation().expect("status").clone();
    let expected = unobserved.worker_request().expect("request");
    status.worker.identity.workflow_id = expected.workflow_id;
    status.worker.identity.job_id = expected.job_id;
    status.lifecycle = InteractiveWorkerLifecycle::Provisioning;
    status.ssh = None;
    let unpinned = fixture
        .store
        .record_remote_worker_recovery(&unobserved, Some(&status))
        .expect("untrusted worker");
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&unpinned.workspace().environment_summary()),
            || panic!("no credentials"),
            |_, _| panic!("no query")
        ),
        Err(RemotePanelStatusError::WorkerUnavailable.into())
    );
    assert!(
        unpinned
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .ssh
            .is_none()
    );
}

#[test]
fn time_limited_workers_are_not_silently_treated_as_persistent() {
    let fixture = fixture(false);
    let mut state = fixture.current().workspace().state().clone();
    state.spec.workspace_local_id = "time-limited".into();
    state.spec.generation = 0;
    state.spec.target.lifetime = crate::cloud_run::WorkerLifetime::TimeLimited { seconds: 600 };
    state.runtime = None;
    let dormant = fixture.store.create_remote_workspace(OWNER, &state).expect("workspace");
    let allocation = fixture
        .store
        .allocate_remote_runtime(&dormant, i64::MAX)
        .expect("allocation");
    let allocation = fixture
        .store
        .reserve_remote_worker_request(&allocation, fixture.recovered.identity().public_key())
        .expect("request");
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&allocation.workspace().environment_summary()),
            || panic!("no credentials"),
            |_, _| panic!("no query")
        ),
        Err(ConfiguredRemotePanelStatusError::InvalidRunPodBinding)
    );
    assert_eq!(
        fixture
            .store
            .load_remote_allocation(OWNER, "time-limited")
            .expect("read"),
        Some(allocation)
    );
}

#[test]
fn credential_failure_is_fixed_and_post_credential_drift_never_reaches_inspection() {
    let fixture = fixture(false);
    let before = fixture.current();
    let result = runpod_with(
        &fixture.store,
        &identities(&fixture),
        &config(),
        request(&before.workspace().environment_summary()),
        || Err(ConfiguredRemotePanelStatusError::RunPodCredentialUnavailable),
        |_, _| panic!("no query"),
    );
    assert_eq!(
        result,
        Err(ConfiguredRemotePanelStatusError::RunPodCredentialUnavailable)
    );
    assert_eq!(
        result.expect_err("missing credential").to_string(),
        "RunPod task inspection requires a valid RUNPOD_API_KEY supplied to the controller"
    );
    assert_eq!(fixture.current(), before);
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&before.workspace().environment_summary()),
            || {
                fixture.edit_intent();
                key()
            },
            |_, _| panic!("drift before provider or SSH")
        ),
        Err(RemoteWorkspaceRecoveryError::StateChanged.into())
    );
}

#[test]
fn failed_exact_observation_or_query_never_becomes_a_status() {
    let fixture = fixture(false);
    let before = fixture.current();
    let mut wrong = fixture.recovered.observation().expect("status").clone();
    wrong.ssh.as_mut().expect("pin").host_key = fixture.recovered.identity().public_key().replace("ssh-ed25519", "bad");
    let observer = ExactObserver {
        status: &wrong,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    assert_eq!(
        runpod_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&before.workspace().environment_summary()),
            key,
            |_, allocation| {
                inspect_remote_allocation(&identities(&fixture), &observer, allocation)?;
                panic!("wrong pin cannot reach SSH")
            }
        ),
        Err(RemoteWorkspaceRecoveryError::InvalidObservation.into())
    );
    for (error, expected) in [
        (RemotePanelStatusError::QueryFailed, RemotePanelStatusError::QueryFailed),
        (RemotePanelStatusError::Deadline, RemotePanelStatusError::Deadline),
        (
            RemotePanelStatusError::InvalidResponse,
            RemotePanelStatusError::InvalidResponse,
        ),
    ] {
        assert_eq!(
            runpod_with(
                &fixture.store,
                &identities(&fixture),
                &config(),
                request(&before.workspace().environment_summary()),
                key,
                |_, _| Err(error.into())
            ),
            Err(expected.into())
        );
    }
    assert_eq!(fixture.current(), before);
}

#[test]
fn corrupted_retained_tuples_fail_during_loading_before_credentials() {
    for fault in 0..4 {
        let fixture = fixture(false);
        let expected = fixture.current().workspace().environment_summary();
        let raw = rusqlite::Connection::open(fixture.store.path()).expect("fixture database");
        let bytes: Vec<u8> = raw
            .query_row(
                "SELECT snapshot FROM remote_workspaces WHERE workspace_local_id='workspace'",
                [],
                |row| row.get(0),
            )
            .expect("snapshot");
        let mut snapshot: serde_json::Value = serde_json::from_slice(&bytes).expect("snapshot JSON");
        let worker = &mut snapshot["state"]["runtime"]["worker"];
        match fault {
            0 => worker["identity"]["workflow_id"] = serde_json::json!("00000000-0000-4000-8000-000000000099"),
            1 => worker["identity"]["job_id"] = serde_json::json!("00000000-0000-4000-8000-000000000099"),
            2 => worker["target"]["disk_gib"] = serde_json::json!(21),
            _ => worker["ssh_public_key"] = serde_json::json!("synthetic-private-corruption"),
        }
        raw.execute(
            "UPDATE remote_workspaces SET snapshot=?1 WHERE workspace_local_id='workspace'",
            [serde_json::to_vec(&snapshot).expect("corrupted fixture")],
        )
        .expect("inject corruption");
        assert_eq!(
            runpod_with(
                &fixture.store,
                &identities(&fixture),
                &config(),
                request(&expected),
                || panic!("no credentials"),
                |_, _| panic!("no inspection")
            ),
            Err(RemoteWorkspaceRecoveryError::StorageUnavailable.into())
        );
    }
}

fn change_independent_record(fixture: &Fixture, selection: bool) {
    let before = fixture.current();
    let mut raw = rusqlite::Connection::open(fixture.store.path()).expect("fixture database");
    let transaction = raw.transaction().expect("atomic fixture change");
    if selection {
        let trigger: String = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                [],
                |row| row.get(0),
            )
            .expect("immutable trigger");
        // Corruption simulation only; no product API may rewrite this immutable record.
        transaction
            .execute_batch("DROP TRIGGER remote_network_volume_selections_no_update")
            .expect("fixture guard");
        transaction.execute("UPDATE remote_network_volume_selections SET volume_id='different_volume' WHERE workspace_local_id='workspace'", [])
            .expect("fixture selection change");
        transaction.execute_batch(&trigger).expect("restore exact schema");
    } else {
        let mut workflow = before.workflow().workflow().clone();
        workflow.retain_until_millis -= 1;
        transaction
            .execute(
                "UPDATE cloud_workflows SET retain_until_millis=?1, snapshot=?2 WHERE workflow_id=?3",
                rusqlite::params![
                    workflow.retain_until_millis,
                    serde_json::to_vec(&workflow).expect("workflow"),
                    workflow.id.to_string()
                ],
            )
            .expect("workflow-only change");
    }
    transaction.commit().expect("fixture change committed");
    assert_eq!(
        fixture.current().workspace(),
        before.workspace(),
        "no workspace revision change"
    );
}

#[test]
fn independent_workflow_and_selection_changes_are_fenced_before_and_after_inspection() {
    for selection in [false, true] {
        for before_inspection in [false, true] {
            let fixture = fixture(true);
            let expected = fixture.current().workspace().environment_summary();
            assert_eq!(
                runpod_with(
                    &fixture.store,
                    &identities(&fixture),
                    &config(),
                    request(&expected),
                    || {
                        if before_inspection {
                            change_independent_record(&fixture, selection);
                        }
                        key()
                    },
                    |_, _| {
                        assert!(
                            !before_inspection,
                            "changed independent records must reject before provider I/O"
                        );
                        change_independent_record(&fixture, selection);
                        Ok(RemotePanelStatus::Running { pid: 123 })
                    }
                ),
                Err(RemoteWorkspaceRecoveryError::StateChanged.into())
            );
        }
    }
}
