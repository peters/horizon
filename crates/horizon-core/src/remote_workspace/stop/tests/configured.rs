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
            docker_host: "unix:///nonexistent-stop-fixture/docker.sock".into(),
        }],
    }
}

fn stop(fixture: &Fixture, config: &RemoteProviderConfig) -> Result<RemoteEnvironmentSummary, ConfiguredStopError> {
    stop_configured_remote_environment(
        &fixture.store,
        config,
        &fixture.current().workspace().environment_summary(),
    )
}

#[test]
fn unsupported_and_unconfigured_providers_cannot_record_stop_intent() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut expected = before.workspace().environment_summary();
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        expected.provider = provider;
        assert_eq!(
            stop_configured_remote_environment(&fixture.store, &RemoteProviderConfig::default(), &expected),
            Err(ConfiguredStopError::UnsupportedProvider)
        );
    }
    assert_eq!(
        stop(&fixture, &RemoteProviderConfig::default()),
        Err(ConfiguredStopError::Configuration(
            RemoteProviderConfigError::UnconfiguredLocalProfile
        ))
    );
    let mut profiles = config();
    profiles.local_docker[0].name = "Development".into();
    assert_eq!(
        stop(&fixture, &profiles),
        Err(ConfiguredStopError::Configuration(
            RemoteProviderConfigError::UnconfiguredLocalProfile
        ))
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn duplicate_and_invalid_profiles_fail_before_intent_without_echoing_values() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut profiles = config();
    profiles.local_docker.push(profiles.local_docker[0].clone());
    assert_eq!(
        stop(&fixture, &profiles),
        Err(ConfiguredStopError::Configuration(
            RemoteProviderConfigError::DuplicateLocalProfile { index: 1 }
        ))
    );
    profiles.local_docker.pop();
    profiles.local_docker[0].docker_host = "tcp://private-endpoint-marker:1234".into();
    let error = stop(&fixture, &profiles).expect_err("nonlocal endpoint");
    assert_eq!(
        error,
        ConfiguredStopError::Configuration(RemoteProviderConfigError::InvalidLocalProfile { index: 0 })
    );
    assert!(!format!("{error:?} {error}").contains("private-endpoint-marker"));
    assert_eq!(fixture.current(), before);
}

#[test]
fn every_changed_selection_field_is_rejected_before_provider_access() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let original = before.workspace().environment_summary();
    let mut changed = vec![original.clone(); 11];
    changed[0].revision += 1;
    changed[1].repository = "another/repository".into();
    changed[2].generation += 1;
    changed[3].panel_count += 1;
    changed[4].saved_phase = Some(RemoteRuntimePhase::Ready);
    changed[5].checkpoint = None;
    changed[6].workflow_id = Some(crate::cloud_run::CloudWorkflowId::new());
    changed[7].job_id = Some(crate::cloud_run::CloudJobId::new());
    changed[8].worker_identity.as_mut().expect("worker").resource_id = "b".repeat(64);
    changed[9].worker_identity = None;
    changed[10].profile = "another-profile".into();
    let mut profiles = config();
    profiles.local_docker.push(LocalDockerProfile {
        name: "another-profile".into(),
        docker_host: profiles.local_docker[0].docker_host.clone(),
    });
    for expected in changed {
        assert_eq!(
            stop_configured_remote_environment(&fixture.store, &profiles, &expected),
            Err(ConfiguredStopError::Stop(Error::StateChanged))
        );
        assert_eq!(fixture.current(), before);
    }
    let mut next = before.workspace().state().clone();
    next.spec.panels[0].task_handoff = Some("changed-private-task-marker".into());
    let updated = fixture
        .store
        .replace_remote_workspace(before.workspace(), &next)
        .expect("changed record");
    assert_eq!(
        stop_configured_remote_environment(&fixture.store, &config(), &original),
        Err(ConfiguredStopError::Stop(Error::StateChanged))
    );
    assert_eq!(fixture.current().workspace(), &updated);
}

#[test]
fn foreign_missing_and_malformed_selection_keys_cannot_mutate_records() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let original = before.workspace().environment_summary();
    let mut foreign = original.clone();
    foreign.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
    assert!(stop_configured_remote_environment(&fixture.store, &config(), &foreign).is_err());
    let mut missing = original.clone();
    missing.workspace_local_id = "missing-workspace".into();
    assert_eq!(
        stop_configured_remote_environment(&fixture.store, &config(), &missing),
        Err(ConfiguredStopError::Stop(Error::MissingAllocation))
    );
    let mut malformed = original;
    malformed.owning_session_id = "private-owner-marker".into();
    let error = stop_configured_remote_environment(&fixture.store, &config(), &malformed).expect_err("owner");
    assert!(!format!("{error:?} {error}").contains("private-owner-marker"));
    assert_eq!(fixture.current(), before);
}

#[test]
fn legacy_management_and_missing_worker_never_become_a_stop_request() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let mut next = before.workspace().state().clone();
    next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::ApplicationExit,
        requested_at_millis: 1,
    });
    let legacy = fixture
        .store
        .replace_remote_workspace(before.workspace(), &next)
        .expect("legacy intent");
    assert_eq!(
        stop(&fixture, &config()),
        Err(ConfiguredStopError::Stop(Error::ManagementConflict))
    );
    assert_eq!(fixture.current().workspace(), &legacy);
    next.spec.workspace_local_id = "dormant".into();
    next.runtime = None;
    next.checkpoint = None;
    let dormant = fixture.store.create_remote_workspace(OWNER, &next).expect("dormant");
    assert_eq!(
        stop_configured_remote_environment(&fixture.store, &config(), &dormant.environment_summary()),
        Err(ConfiguredStopError::Stop(Error::MissingAllocation))
    );
    let unobserved = fixture
        .store
        .allocate_remote_runtime(&dormant, i64::MAX)
        .expect("allocate");
    assert_eq!(
        stop_configured_remote_environment(&fixture.store, &config(), &unobserved.workspace().environment_summary()),
        Err(ConfiguredStopError::Stop(Error::MissingWorker))
    );
    assert_eq!(
        fixture.store.load_remote_allocation(OWNER, "dormant").expect("read"),
        Some(unobserved)
    );
}

#[test]
fn unavailable_explicit_endpoint_retains_intent_and_requires_a_refreshed_retry() {
    let fixture = Fixture::new();
    let before = fixture.current();
    let selected = before.workspace().environment_summary();
    assert_eq!(
        stop(&fixture, &config()),
        Err(ConfiguredStopError::Stop(Error::ProviderUnavailable))
    );
    let stopping = fixture.current();
    assert!(matches!(fixture.phase(), RemoteRuntimePhase::Stopping { .. }));
    let mut expected = before.workspace().state().clone();
    expected.runtime.as_mut().expect("runtime").phase = fixture.phase();
    assert_eq!(stopping.workspace().state(), &expected);
    assert_eq!(stopping.workspace().revision(), before.workspace().revision() + 1);
    assert_eq!(stopping.workflow(), before.workflow());
    let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("fresh client");
    assert_eq!(
        stop_configured_remote_environment(&reopened, &config(), &selected),
        Err(ConfiguredStopError::Stop(Error::StateChanged))
    );
    assert_eq!(
        stop_configured_remote_environment(&reopened, &config(), &stopping.workspace().environment_summary()),
        Err(ConfiguredStopError::Stop(Error::ProviderUnavailable))
    );
    assert_eq!(fixture.current(), stopping);
}
