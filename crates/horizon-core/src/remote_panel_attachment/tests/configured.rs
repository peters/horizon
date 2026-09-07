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
    }
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
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        expected.provider = provider;
        assert_eq!(
            connect(&fixture, &RemoteProviderConfig::default(), &expected, OWNER, "terminal"),
            ConfiguredRemotePanelAttachError::UnsupportedProvider
        );
    }
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
