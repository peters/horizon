use super::super::configured::inspect_with as configured_with;
use super::*;
use crate::remote_workspace_recovery::RemoteWorkspaceRecoveryError;
use crate::{cloud_run::local_docker::LocalDockerProfile, remote_provider_config::RemoteProviderConfig};

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: vec![LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///unused-task-check/socket".into(),
        }],
        ..Default::default()
    }
}

fn identities(fixture: &Fixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")))
}

fn request(expected: &crate::remote_workspace::RemoteEnvironmentSummary) -> ConfiguredRemotePanelStatusRequest<'_> {
    ConfiguredRemotePanelStatusRequest {
        expected,
        client_session_id: OWNER,
        panel_id: "terminal",
    }
}

fn check(
    fixture: &Fixture,
    query: impl FnOnce() -> Result<Vec<u8>, RemotePanelStatusError>,
) -> Result<RemotePanelObservation, ConfiguredRemotePanelStatusError> {
    configured_with(
        &fixture.store,
        &identities(fixture),
        &config(),
        request(&fixture.current().workspace().environment_summary()),
        |profile| {
            assert_eq!(profile, &config().local_docker[0]);
            Ok(Provider(fixture.recovered.observation().expect("observation").clone()))
        },
        |store, recovered, panel| {
            super::super::inspect_with(store, recovered, panel, Inspection::Status, |_, _, _| query())
        },
    )
}

#[test]
fn selected_admission_rejects_before_provider_or_ssh_access() {
    let fixture = Fixture::new();
    let before = fixture.current();
    for fault in 0..8 {
        let mut expected = before.workspace().environment_summary();
        let mut config = config();
        let mut owner = OWNER;
        let mut panel = "terminal";
        let error = match fault {
            0 => {
                owner = "copied-owner";
                ConfiguredRemotePanelStatusError::ClientSessionMismatch
            }
            1 => {
                expected.provider = CloudProvider::Azure;
                ConfiguredRemotePanelStatusError::UnsupportedProvider
            }
            2 => {
                config.local_docker.clear();
                crate::remote_provider_config::RemoteProviderConfigError::UnconfiguredLocalProfile.into()
            }
            3 => {
                config.local_docker[0].name = "Development".into();
                crate::remote_provider_config::RemoteProviderConfigError::UnconfiguredLocalProfile.into()
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
                expected.generation += 1;
                RemoteWorkspaceRecoveryError::StateChanged.into()
            }
            _ => {
                panel = "unknown;start";
                RemotePanelStatusError::UnknownPanel.into()
            }
        };
        assert_eq!(
            configured_with::<Provider>(
                &fixture.store,
                &identities(&fixture),
                &config,
                ConfiguredRemotePanelStatusRequest {
                    expected: &expected,
                    client_session_id: owner,
                    panel_id: panel
                },
                |_| panic!("must not construct provider"),
                |_, _, _| panic!("must not query"),
            ),
            Err(error)
        );
    }
    assert_eq!(fixture.current(), before);
}

#[test]
fn exact_selected_status_preserves_snapshots_keys_and_unknown_exit_semantics() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
    for (response, status) in [
        (RUNNING, RemotePanelStatus::Running { pid: 123 }),
        (
            br#"{"state":"exited","panel":"terminal","pid":123,"exit_status":42}"#.as_slice(),
            RemotePanelStatus::Exited {
                pid: 123,
                exit_status: Some(42),
            },
        ),
        (
            br#"{"state":"exited","panel":"terminal","pid":123,"exit_status":null}"#.as_slice(),
            RemotePanelStatus::Exited {
                pid: 123,
                exit_status: None,
            },
        ),
        (
            br#"{"state":"unavailable","panel":"terminal"}"#.as_slice(),
            RemotePanelStatus::Unavailable,
        ),
    ] {
        let observation = check(&fixture, || Ok(response.to_vec())).expect("task check");
        assert_eq!(observation.panel_id, "terminal");
        assert_eq!(observation.status, status);
        assert!(observation.observed_at_millis > 0);
        assert_eq!(fixture.current(), before);
    }
    assert_eq!(
        std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
        key
    );
    let path = fixture.recovered.identity().private_key_path();
    let retained = path.with_extension("retained-fixture");
    std::fs::rename(path, &retained).expect("hide exact fixture key");
    assert!(matches!(
        check(&fixture, || panic!("no SSH without key")),
        Err(ConfiguredRemotePanelStatusError::Recovery(
            RemoteWorkspaceRecoveryError::Identity(_)
        ))
    ));
    assert!(!path.exists(), "no replacement identity");
    assert_eq!(std::fs::read(retained).expect("retained key"), key);
    assert_eq!(fixture.current(), before);
}

#[test]
fn changes_during_provider_or_query_io_reject_the_observation() {
    for during_query in [false, true] {
        let fixture = Fixture::new();
        let expected = fixture.current().workspace().environment_summary();
        assert_eq!(
            configured_with(
                &fixture.store,
                &identities(&fixture),
                &config(),
                request(&expected),
                |_| {
                    if !during_query {
                        fixture.edit_intent();
                    }
                    Ok(Provider(fixture.recovered.observation().expect("observation").clone()))
                },
                |store, recovered, panel| super::super::inspect_with(
                    store,
                    recovered,
                    panel,
                    Inspection::Status,
                    |_, _, _| {
                        assert!(during_query, "provider drift must reject before SSH");
                        fixture.edit_intent();
                        Ok(RUNNING.to_vec())
                    }
                ),
            ),
            Err(RemotePanelStatusError::StateChanged.into())
        );
        assert_eq!(fixture.current().workspace().state().spec.working_directory, "src");
    }
}

#[test]
fn pending_management_and_unrecorded_pins_are_not_recovered_or_repaired() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut state = before.workspace().state().clone();
    state.spec.workspace_local_id = "unobserved".into();
    state.spec.generation = 0;
    state.runtime = None;
    let dormant = fixture.store.create_remote_workspace(OWNER, &state).expect("dormant");
    let allocation = fixture
        .store
        .allocate_remote_runtime(&dormant, i64::MAX)
        .expect("allocation");
    let allocation = fixture
        .store
        .reserve_remote_worker_request(&allocation, fixture.recovered.identity().public_key())
        .expect("request");
    let stopped = fixture
        .store
        .record_remote_stop_phase(
            &before,
            crate::remote_workspace::RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        )
        .expect("stop intent");
    for (allocation, expected_error) in [
        (allocation, RemotePanelStatusError::WorkerUnavailable.into()),
        (stopped, RemoteWorkspaceRecoveryError::ManagementPending.into()),
    ] {
        assert_eq!(
            configured_with::<Provider>(
                &fixture.store,
                &identities(&fixture),
                &config(),
                request(&allocation.workspace().environment_summary()),
                |_| panic!("no provider"),
                |_, _, _| panic!("no SSH")
            ),
            Err(expected_error)
        );
        assert_eq!(
            fixture
                .store
                .load_remote_allocation(OWNER, &allocation.workspace().state().spec.workspace_local_id)
                .expect("load"),
            Some(allocation)
        );
    }
}

#[test]
fn nonready_or_wrong_pinned_worker_and_query_failures_do_not_become_task_results() {
    let fixture = Fixture::with_lifecycle(InteractiveWorkerLifecycle::Stopped);
    assert_eq!(
        check(&fixture, || panic!("no stopped-worker SSH")),
        Err(RemotePanelStatusError::WorkerUnavailable.into())
    );
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut wrong = fixture.recovered.observation().expect("observation").clone();
    wrong.ssh.as_mut().expect("pin").port += 1;
    assert_eq!(
        configured_with(
            &fixture.store,
            &identities(&fixture),
            &config(),
            request(&before.workspace().environment_summary()),
            |_| Ok(Provider(wrong)),
            |_, _, _| panic!("no mismatched-pin SSH")
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
        assert_eq!(check(&fixture, || Err(error)), Err(expected.into()));
    }
    assert_eq!(fixture.current(), before);
}
