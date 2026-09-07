use super::*;
use crate::{
    cloud_run::local_docker::LocalDockerProfile,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
};

fn config() -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: vec![LocalDockerProfile {
            name: "development".into(),
            docker_host: "unix:///nonexistent-observation-fixture/docker.sock".into(),
        }],
    }
}

fn check(
    fixture: &Fixture,
    config: &RemoteProviderConfig,
) -> Result<RemoteEnvironmentObservation, ConfiguredObservationError> {
    observe_configured_remote_environment(
        &fixture.store,
        config,
        &fixture.reload().workspace().environment_summary(),
    )
}

#[test]
fn configuration_rejects_missing_duplicate_and_invalid_profiles_without_mutation() {
    let fixture = Fixture::new();
    let before = fixture.reload();
    let counts = fixture.counts();
    assert_eq!(
        check(&fixture, &RemoteProviderConfig::default()),
        Err(ConfiguredObservationError::Configuration(
            RemoteProviderConfigError::UnconfiguredLocalProfile
        ))
    );
    let mut profiles = config();
    profiles.local_docker.push(profiles.local_docker[0].clone());
    assert_eq!(
        check(&fixture, &profiles),
        Err(ConfiguredObservationError::Configuration(
            RemoteProviderConfigError::DuplicateLocalProfile { index: 1 }
        ))
    );
    profiles.local_docker.pop();
    profiles.local_docker[0].docker_host = "tcp://private-endpoint-marker:1234".into();
    let error = check(&fixture, &profiles).expect_err("nonlocal endpoint");
    assert!(!format!("{error:?} {error}").contains("private-endpoint-marker"));
    assert_eq!(fixture.reload(), before);
    assert_eq!(fixture.counts(), counts);
}

#[test]
fn stale_or_foreign_summary_is_rejected_before_provider_access() {
    let fixture = Fixture::new();
    let before = fixture.reload();
    let expected = before.workspace().environment_summary();
    let mut changes = vec![expected.clone(); 4];
    changes[0].revision += 1;
    changes[1].repository = "another/repository".into();
    changes[2].generation += 1;
    changes[3].panel_count += 1;
    for changed in changes {
        assert_eq!(
            observe_configured_remote_environment(&fixture.store, &config(), &changed),
            Err(ConfiguredObservationError::Observation(Error::StateChanged))
        );
    }
    let mut foreign = expected;
    foreign.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
    assert!(observe_configured_remote_environment(&fixture.store, &config(), &foreign).is_err());
    assert_eq!(fixture.reload(), before);
}

#[test]
fn unsupported_provider_and_unreserved_allocation_fail_closed() {
    let fixture = Fixture::with_request(WorkerLifetime::Persistent, false);
    assert_eq!(
        check(&fixture, &config()),
        Err(ConfiguredObservationError::Observation(Error::MissingRequest))
    );
    let mut expected = fixture.reload().workspace().environment_summary();
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        expected.provider = provider;
        assert_eq!(
            observe_configured_remote_environment(&fixture.store, &RemoteProviderConfig::default(), &expected),
            Err(ConfiguredObservationError::UnsupportedProvider)
        );
    }
}

#[test]
fn unavailable_explicit_endpoint_does_not_change_saved_state() {
    let fixture = Fixture::new();
    let before = fixture.reload();
    let counts = fixture.counts();
    assert_eq!(
        check(&fixture, &config()),
        Err(ConfiguredObservationError::Observation(Error::ProviderUnavailable))
    );
    assert_eq!(fixture.reload(), before);
    assert_eq!(fixture.counts(), counts);
}
