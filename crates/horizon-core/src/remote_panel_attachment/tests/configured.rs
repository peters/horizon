use super::*;
use crate::{
    cloud_run::local_docker::LocalDockerProfile,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::RemoteEnvironmentSummary,
};

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: vec![LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///nonexistent-panel-attachment-fixture/docker.sock".into(),
        }],
        ..Default::default()
    }
}

fn runpod_fixture(network: bool) -> (Fixture, StoredRemoteAllocation, crate::cloud_run::runpod::RunPodProfile) {
    runpod_fixture_variant(network, None)
}

fn runpod_fixture_variant(
    network: bool,
    fault: Option<u8>,
) -> (Fixture, StoredRemoteAllocation, crate::cloud_run::runpod::RunPodProfile) {
    use crate::cloud_run::runpod::RunPodNetworkVolumeExpectation;
    let fixture = Fixture::new();
    let mut state = fixture.current().workspace().state().clone();
    state.spec.workspace_local_id = "runpod-workspace".into();
    state.spec.generation = 0;
    state.spec.target.provider = CloudProvider::RunPod;
    state.runtime = None;
    if fault == Some(4) {
        state.spec.panels[0].task_handoff = Some("synthetic handoff".into());
    }
    if fault == Some(5) {
        state.spec.panels[0].kind = crate::PanelKind::Shell;
        state.spec.panels[0].command = None;
    }
    let saved = fixture
        .store
        .create_remote_workspace(OWNER, &state)
        .expect("RunPod workspace");
    let allocation = fixture
        .store
        .allocate_remote_runtime(&saved, i64::MAX)
        .expect("allocation");
    if network {
        fixture
            .store
            .record_remote_network_volume_selection(
                &allocation,
                &RunPodNetworkVolumeExpectation {
                    volume_id: "synthetic_volume".into(),
                    data_center_id: "EU-RO-1".into(),
                    minimum_size_gb: 10,
                },
            )
            .expect("immutable selection before key/claim");
    }
    let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
    let identity = fixture
        .identities
        .prepare_new(runtime.workflow_id, runtime.job_id)
        .expect("identity");
    let reserved = fixture
        .store
        .reserve_remote_worker_request(&allocation, identity.public_key())
        .expect("request");
    let request = reserved.worker_request().expect("request");
    let mut status = InteractiveWorkerStatus {
        worker: InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "syntheticpod".into(),
            },
            target: request.target,
            ssh_public_key: request.ssh_public_key,
            lifetime: InteractiveWorkerLifetime::Persistent,
        },
        lifecycle: InteractiveWorkerLifecycle::Ready,
        ssh: Some(InteractiveWorkerSshEndpoint {
            host: "127.0.0.1".into(),
            port: 2222,
            username: "root".into(),
            host_key: identity.public_key().into(),
        }),
    };
    if fault == Some(2) {
        status.lifecycle = InteractiveWorkerLifecycle::Provisioning;
        status.ssh = None;
    }
    let saved = fixture
        .store
        .record_remote_worker_recovery(&reserved, Some(&status))
        .expect("retained observation");
    let profile = serde_json::from_value(serde_json::json!({
        "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
        "ports":["22/tcp"], "volume_gib":0, "data_center_id":"EU-RO-1"
    }))
    .expect("profile");
    (fixture, saved, profile)
}

#[test]
fn ordinary_and_selected_runpod_reuse_retained_trust_without_mutation() {
    use crate::cloud_run::runpod::RunPodApiKey;
    for network in [false, true] {
        let (fixture, allocation, profile) = runpod_fixture(network);
        let mut expired = allocation.workflow().workflow().clone();
        expired.created_at_millis = 1000;
        expired.updated_at_millis = 1000;
        expired.retain_until_millis = 2000;
        rusqlite::Connection::open(fixture.store.path())
            .expect("fixture database")
            .execute(
                "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
             retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
                rusqlite::params![serde_json::to_vec(&expired).expect("snapshot"), expired.id.to_string()],
            )
            .expect("expired setup fixture");
        let allocation = fixture
            .store
            .load_remote_allocation(OWNER, "runpod-workspace")
            .expect("expired setup still retained")
            .expect("allocation");
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        let identity = fixture
            .identities
            .recover(
                runtime.workflow_id,
                runtime.job_id,
                runtime.ssh_public_key.as_deref().expect("key"),
            )
            .expect("identity");
        let bytes = std::fs::read(identity.private_key_path()).expect("key bytes");
        let selection = fixture
            .store
            .load_remote_network_volume_selection(&allocation)
            .expect("selection");
        super::super::configured::runpod_with(
            &fixture.store,
            &fixture.identities,
            &profile,
            Fixture::request(&allocation),
            || {
                RunPodApiKey::new("synthetic-private-marker")
                    .map_err(|_| ConfiguredRemotePanelAttachError::RunPodCredentialUnavailable)
            },
            |provider, admitted| {
                assert_eq!(provider.provider(), CloudProvider::RunPod);
                assert_eq!(admitted.allocation, &allocation);
                // A mismatched complete worker is rejected before any HTTP call.
                if network {
                    let mut wrong = runtime.worker.clone().expect("worker");
                    wrong.target.profile = "different".into();
                    assert!(provider.inspect_worker(&wrong).is_err());
                }
                Ok(())
            },
        )
        .expect("provider construction and handoff");
        assert_eq!(
            fixture
                .store
                .load_remote_allocation(OWNER, "runpod-workspace")
                .expect("load"),
            Some(allocation.clone())
        );
        assert_eq!(
            fixture
                .store
                .load_remote_network_volume_selection(&allocation)
                .expect("selection"),
            selection
        );
        assert_eq!(std::fs::read(identity.private_key_path()).expect("key bytes"), bytes);
    }
}

#[test]
fn runpod_missing_credential_is_fixed_and_does_not_reach_attachment() {
    let (fixture, allocation, profile) = runpod_fixture(true);
    let error = super::super::configured::runpod_with::<()>(
        &fixture.store,
        &fixture.identities,
        &profile,
        Fixture::request(&allocation),
        || Err(ConfiguredRemotePanelAttachError::RunPodCredentialUnavailable),
        |_, _| panic!("no provider or SSH I/O without credential"),
    )
    .expect_err("missing key");
    assert_eq!(error, ConfiguredRemotePanelAttachError::RunPodCredentialUnavailable);
    assert_eq!(
        error.to_string(),
        "RunPod reconnection requires a valid RUNPOD_API_KEY supplied to the controller"
    );
    assert!(!format!("{error:?}").contains("synthetic-private-marker"));
}

#[test]
fn state_change_during_credential_lookup_discards_before_attachment() {
    let (fixture, allocation, profile) = runpod_fixture(true);
    let error = super::super::configured::runpod_with::<()>(
        &fixture.store,
        &fixture.identities,
        &profile,
        Fixture::request(&allocation),
        || {
            let mut next = allocation.workspace().state().clone();
            next.spec.working_directory = "changed-during-lookup".into();
            fixture
                .store
                .replace_remote_workspace(allocation.workspace(), &next)
                .expect("concurrent edit");
            crate::cloud_run::runpod::RunPodApiKey::new("synthetic-private-marker")
                .map_err(|_| ConfiguredRemotePanelAttachError::RunPodCredentialUnavailable)
        },
        |_, _| panic!("stale snapshot cannot reach attachment"),
    )
    .expect_err("state drift");
    assert_eq!(error, RemotePanelAttachError::StateChanged.into());
}

#[test]
fn runpod_local_admission_failures_precede_credentials() {
    for fault in 0..8 {
        let (fixture, allocation, mut profile) = runpod_fixture_variant(true, Some(fault));
        let mut state = allocation.workspace().state().clone();
        match fault {
            0 => profile.gpu_count = 0,
            1 => profile.data_center_id = Some("EU-OTHER-1".into()),
            3 => {
                state.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Stopping { requested_at_millis: 1 }
            }
            6 => state.spec.working_directory = "changed".into(),
            2 | 4 | 5 | 7 => {}
            _ => unreachable!(),
        }
        if matches!(fault, 3 | 6) {
            fixture
                .store
                .replace_remote_workspace(allocation.workspace(), &state)
                .expect("fixture edit");
        }
        let current = fixture
            .store
            .load_remote_allocation(OWNER, "runpod-workspace")
            .expect("load")
            .expect("allocation");
        let request = RemotePanelAttachRequest {
            allocation: if fault == 6 { &allocation } else { &current },
            panel_id: if fault == 7 { "missing" } else { "terminal" },
            terminal: terminal_size(),
        };
        let error = super::super::configured::runpod_with::<()>(
            &fixture.store,
            &fixture.identities,
            &profile,
            request,
            || panic!("credentials must follow local admission"),
            |_, _| panic!("no attachment"),
        )
        .expect_err("invalid local admission");
        match fault {
            0 | 1 => assert_eq!(error, ConfiguredRemotePanelAttachError::InvalidRunPodBinding),
            2 => assert_eq!(
                error,
                RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable).into()
            ),
            4 | 5 => assert_eq!(
                error,
                RemotePanelAttachError::from(RemotePanelStatusError::UnsupportedIntent).into()
            ),
            7 => assert_eq!(
                error,
                RemotePanelAttachError::from(RemotePanelStatusError::UnknownPanel).into()
            ),
            _ => {}
        }
        assert_eq!(
            fixture
                .store
                .load_remote_allocation(OWNER, "runpod-workspace")
                .expect("load"),
            Some(current)
        );
    }
}

#[test]
fn missing_retained_key_is_not_recreated_before_credential_lookup() {
    let (fixture, allocation, profile) = runpod_fixture(false);
    let absent_home = HorizonHome::from_root(fixture.directory.path().join("absent-identity-home"));
    let absent = RemoteSshIdentityStore::new(&absent_home);
    assert!(
        super::super::configured::runpod_with::<()>(
            &fixture.store,
            &absent,
            &profile,
            Fixture::request(&allocation),
            || panic!("no credential lookup"),
            |_, _| panic!("no attachment")
        )
        .is_err()
    );
    assert!(!fixture.directory.path().join("absent-identity-home").exists());
}

#[test]
fn selected_runpod_config_and_summary_are_admitted_before_credentials() {
    let (fixture, allocation, profile) = runpod_fixture(true);
    let expected = allocation.workspace().environment_summary();
    let configured = RemoteProviderConfig {
        runpod: vec![profile],
        ..Default::default()
    };
    assert_eq!(
        connect(&fixture, &configured, &expected, "copied-owner", "terminal"),
        ConfiguredRemotePanelAttachError::ClientSessionMismatch
    );
    assert_eq!(
        connect(&fixture, &RemoteProviderConfig::default(), &expected, OWNER, "terminal"),
        ConfiguredRemotePanelAttachError::Configuration(RemoteProviderConfigError::UnconfiguredRunPodProfile)
    );
    let mut stale = expected;
    stale.revision += 1;
    assert_eq!(
        connect(&fixture, &configured, &stale, OWNER, "terminal"),
        RemoteWorkspaceRecoveryError::StateChanged.into()
    );
}

fn connect(
    fixture: &Fixture,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
    owner: &str,
    panel: &str,
) -> ConfiguredRemotePanelAttachError {
    attach_configured_remote_panel(
        &fixture.store,
        &fixture.identities,
        config,
        ConfiguredRemotePanelAttachRequest {
            expected,
            client_session_id: owner,
            panel_id: panel,
            terminal: terminal_size(),
        },
    )
    .expect_err("synthetic fixture cannot admit an interactive connection")
}

#[test]
fn actual_owner_and_supported_provider_are_required_before_profile_access() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut expected = before.workspace().environment_summary();
    assert_eq!(
        connect(
            &fixture,
            &RemoteProviderConfig::default(),
            &expected,
            "copied-session",
            "terminal"
        ),
        ConfiguredRemotePanelAttachError::ClientSessionMismatch
    );
    expected.provider = CloudProvider::Azure;
    assert_eq!(
        connect(&fixture, &RemoteProviderConfig::default(), &expected, OWNER, "terminal"),
        ConfiguredRemotePanelAttachError::UnsupportedProvider
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn exact_valid_profile_is_required_without_ambient_fallback_or_diagnostic_leakage() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let expected = before.workspace().environment_summary();
    let missing = ConfiguredRemotePanelAttachError::Configuration(RemoteProviderConfigError::UnconfiguredLocalProfile);
    assert_eq!(
        connect(&fixture, &RemoteProviderConfig::default(), &expected, OWNER, "terminal"),
        missing
    );
    let mut profiles = config();
    profiles.local_docker[0].name = "Development".into();
    assert_eq!(connect(&fixture, &profiles, &expected, OWNER, "terminal"), missing);
    let mut profiles = config();
    profiles.local_docker.push(profiles.local_docker[0].clone());
    assert_eq!(
        connect(&fixture, &profiles, &expected, OWNER, "terminal"),
        ConfiguredRemotePanelAttachError::Configuration(RemoteProviderConfigError::DuplicateLocalProfile { index: 1 })
    );
    profiles.local_docker.pop();
    profiles.local_docker[0].docker_host = "tcp://private-endpoint-marker:2375".into();
    let error = connect(&fixture, &profiles, &expected, OWNER, "terminal");
    assert!(matches!(error, ConfiguredRemotePanelAttachError::Configuration(_)));
    assert!(!format!("{error:?} {error}").contains("private-endpoint-marker"));
    assert_eq!(fixture.current(), before);
}

#[test]
fn every_stale_summary_or_foreign_owner_rejects_before_provider_access() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let expected = before.workspace().environment_summary();
    let mut changed = vec![expected.clone(); 5];
    changed[0].revision += 1;
    changed[1].generation += 1;
    changed[2].repository = "different/repository".into();
    changed[3].panel_count += 1;
    changed[4].saved_phase = Some(RemoteRuntimePhase::Failed);
    for selection in changed {
        assert_eq!(
            connect(&fixture, &config(), &selection, OWNER, "terminal"),
            RemoteWorkspaceRecoveryError::StateChanged.into()
        );
    }
    let mut foreign = expected;
    foreign.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
    assert_eq!(
        connect(&fixture, &config(), &foreign, OWNER, "terminal"),
        ConfiguredRemotePanelAttachError::ClientSessionMismatch
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn missing_allocation_or_unknown_panel_never_starts_a_replacement() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let expected = before.workspace().environment_summary();
    assert_eq!(
        connect(&fixture, &config(), &expected, OWNER, "missing-panel"),
        ConfiguredRemotePanelAttachError::Attachment(RemotePanelStatusError::UnknownPanel.into())
    );
    let mut unallocated = before.workspace().state().clone();
    unallocated.spec.workspace_local_id = "unallocated-workspace".into();
    unallocated.spec.generation = 0;
    unallocated.runtime = None;
    let saved = fixture
        .store
        .create_remote_workspace(OWNER, &unallocated)
        .expect("unallocated fixture");
    assert_eq!(
        connect(&fixture, &config(), &saved.environment_summary(), OWNER, "terminal"),
        RemoteWorkspaceRecoveryError::MissingAllocation.into()
    );
    assert_eq!(fixture.current(), before);
    assert_eq!(
        fixture
            .store
            .load_remote_workspace(OWNER, "unallocated-workspace")
            .expect("load"),
        Some(saved)
    );
}

#[test]
fn timed_execution_is_rejected_before_provider_access() {
    let fixture = Fixture::with_lifetime(WorkerLifetime::TimeLimited { seconds: 300 });
    let before = fixture.current();
    assert_eq!(
        connect(
            &fixture,
            &config(),
            &before.workspace().environment_summary(),
            OWNER,
            "terminal"
        ),
        RemotePanelAttachError::UnsupportedLifetime.into()
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn unavailable_explicit_endpoint_preserves_allocation_and_retained_key() {
    let fixture = Fixture::with_resource_id(WorkerLifetime::Persistent, &"a".repeat(64));
    let before = fixture.current();
    let runtime = before.workspace().state().runtime.as_ref().expect("runtime");
    let identity = fixture
        .identities
        .recover(
            runtime.workflow_id,
            runtime.job_id,
            runtime.ssh_public_key.as_deref().expect("public key"),
        )
        .expect("retained key");
    let bytes = std::fs::read(identity.private_key_path()).expect("original key");
    assert_eq!(
        connect(
            &fixture,
            &config(),
            &before.workspace().environment_summary(),
            OWNER,
            "terminal"
        ),
        RemoteWorkspaceRecoveryError::ProviderUnavailable.into()
    );
    assert_eq!(fixture.current(), before);
    assert_eq!(std::fs::read(identity.private_key_path()).expect("retained key"), bytes);
}
