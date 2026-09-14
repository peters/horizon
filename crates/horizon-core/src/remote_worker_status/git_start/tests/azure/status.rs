//! Task-status reads reuse the synthetic Azure allocation and provider fixture.
use super::*;
use crate::remote_worker_status::{
    ConfiguredRemotePanelStatusError as Error, RemotePanelObservation, RemotePanelStatusError,
    configured::azure::inspect_with,
};

fn identities(fixture: &AzureFixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")))
}

fn request(expected: &RemoteEnvironmentSummary) -> ConfiguredRemotePanelStatusRequest<'_> {
    ConfiguredRemotePanelStatusRequest {
        expected,
        client_session_id: OWNER,
        panel_id: "shell",
    }
}

fn query(fixture: &AzureFixture, response: &[u8]) -> Result<RemotePanelObservation, Error> {
    let keys = identities(fixture);
    inspect_with(
        &fixture.store,
        &keys,
        &fixture.profile,
        request(&fixture.current().workspace().environment_summary()),
        |_| Ok(fixture.provider()),
        |provider, allocation| {
            let recovered = crate::remote_workspace_recovery::inspect_remote_allocation(&keys, provider, allocation)?;
            Ok(crate::remote_worker_status::inspect_with(
                &fixture.store,
                &recovered,
                "shell",
                crate::remote_worker_status::Inspection::Status,
                |_, _, _| Ok(response.to_vec()),
            )?)
        },
    )
}

#[test]
fn azure_status_reads_exact_task_without_changing_allocation_or_key() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let saved = fixture.current();
    let worker = saved.worker_request().unwrap();
    let identity = identities(&fixture)
        .recover(worker.workflow_id, worker.job_id, &worker.ssh_public_key)
        .unwrap();
    let key = std::fs::read(identity.private_key_path()).unwrap();
    for (response, expected) in [
        (RUNNING.as_bytes(), RemotePanelStatus::Running { pid: 12 }),
        (
            br#"{"state":"exited","panel":"shell","pid":12,"exit_status":null}"#.as_slice(),
            RemotePanelStatus::Exited {
                pid: 12,
                exit_status: None,
            },
        ),
        (
            br#"{"state":"unavailable","panel":"shell"}"#.as_slice(),
            RemotePanelStatus::Unavailable,
        ),
    ] {
        let observed = query(&fixture, response).unwrap();
        assert_eq!(observed.panel_id, "shell");
        assert_eq!(observed.status, expected);
        assert!(observed.observed_at_millis > 0);
        assert_eq!(fixture.current(), saved);
        assert_eq!(std::fs::read(identity.private_key_path()).unwrap(), key);
    }
}

#[test]
fn azure_status_refuses_foreign_stale_unknown_and_drifted_selections_before_client() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    for fault in 0..4 {
        let mut expected = fixture.current().workspace().environment_summary();
        let mut profile = fixture.profile.clone();
        let owner = if fault == 0 { "foreign" } else { OWNER };
        if fault == 1 {
            expected.revision += 1;
        }
        if fault == 3 {
            profile.vm_size = "Standard_D2s_v3".into();
        }
        let result = inspect_with::<AzureProvider>(
            &fixture.store,
            &identities(&fixture),
            &profile,
            ConfiguredRemotePanelStatusRequest {
                expected: &expected,
                client_session_id: owner,
                panel_id: if fault == 2 { "unknown" } else { "shell" },
            },
            |_| panic!("no client for invalid selection"),
            |_, _| panic!("no task query"),
        );
        assert!(result.is_err());
    }
    assert_eq!(fixture.current(), fixture.allocation);
}

#[test]
fn azure_status_refuses_missing_pin_binding_and_private_key_before_client() {
    for fault in 0..3 {
        let fixture = AzureFixture::new(WorkerLifetime::Persistent, fault != 0, fault != 1);
        let keys = identities(&fixture);
        if fault == 2 {
            let request = fixture.allocation.worker_request().unwrap();
            let identity = keys
                .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
                .unwrap();
            std::fs::remove_file(identity.private_key_path()).unwrap();
        }
        let result = inspect_with::<AzureProvider>(
            &fixture.store,
            &keys,
            &fixture.profile,
            request(&fixture.current().workspace().environment_summary()),
            |_| panic!("no client without retained binding and identity"),
            |_, _| panic!("no task query"),
        );
        assert!(result.is_err());
        assert_eq!(fixture.current(), fixture.allocation);
    }
}

#[test]
fn azure_status_does_not_start_stopped_or_transitioning_compute() {
    let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
    let mut current = fixture.current();
    for phase in [
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
        RemoteRuntimePhase::Starting { requested_at_millis: 3 },
    ] {
        current = if matches!(phase, RemoteRuntimePhase::Starting { .. }) {
            fixture.store.record_remote_start_phase(&current, phase).unwrap()
        } else {
            fixture.store.record_remote_stop_phase(&current, phase).unwrap()
        };
        let result = inspect_with::<AzureProvider>(
            &fixture.store,
            &identities(&fixture),
            &fixture.profile,
            request(&current.workspace().environment_summary()),
            |_| panic!("no client for unavailable compute"),
            |_, _| panic!("no task query"),
        );
        assert!(matches!(
            result,
            Err(Error::Inspection(
                RemotePanelStatusError::ManagementPending | RemotePanelStatusError::WorkerUnavailable
            ))
        ));
        assert_eq!(fixture.current(), current);
    }
}

fn change_directory(fixture: &AzureFixture) {
    let saved = fixture.current();
    let mut state = saved.workspace().state().clone();
    state.spec.working_directory = "changed".into();
    fixture
        .store
        .replace_remote_workspace(saved.workspace(), &state)
        .unwrap();
}

#[test]
fn azure_status_discards_client_and_query_results_after_saved_state_drift() {
    for during_client in [true, false] {
        let fixture = AzureFixture::new(WorkerLifetime::Persistent, true, true);
        let result = inspect_with(
            &fixture.store,
            &identities(&fixture),
            &fixture.profile,
            request(&fixture.current().workspace().environment_summary()),
            |_| {
                if during_client {
                    change_directory(&fixture);
                }
                Ok(fixture.provider())
            },
            |_, _| {
                assert!(!during_client);
                change_directory(&fixture);
                Ok(RemotePanelStatus::Running { pid: 12 })
            },
        );
        assert!(matches!(
            result,
            Err(Error::Azure(ConfiguredStopConfirmationError::Stop(
                crate::remote_workspace::stop::RemoteWorkspaceStopError::StateChanged
            )))
        ));
    }
}
