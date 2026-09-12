use super::*;
use crate::cloud_run::{
    CloudWorkflowStore,
    interactive_worker::valid_worker_profile_name,
    local_docker::{LocalDockerError, LocalDockerInteractiveWorkerProvider},
};

mod azure;
mod config;

fn runpod_profile(name: &str) -> RunPodProfile {
    serde_json::from_value(serde_json::json!({
        "name": name, "gpu_type_ids": ["synthetic-gpu"], "gpu_count": 1,
        "ports": ["22/tcp"], "volume_gib": 0, "data_center_id": "EU-RO-1"
    }))
    .expect("synthetic placement")
}

#[test]
fn runpod_profiles_are_explicit_case_sensitive_and_provider_scoped() {
    let mut configured = config(vec![profile("saved", "unix:///unused.sock")]);
    configured.runpod.push(runpod_profile("saved"));
    assert_eq!(configured.validate(), Ok(()));
    assert_eq!(configured.runpod_profile("saved"), Ok(&configured.runpod[0]));
    for name in ["Saved", " saved", "", "RUNPOD_API_KEY"] {
        assert_eq!(
            configured.runpod_profile(name),
            Err(RemoteProviderConfigError::UnconfiguredRunPodProfile)
        );
    }
    let yaml = serde_yaml::to_string(&configured).expect("serialize");
    assert_eq!(
        serde_yaml::from_str::<RemoteProviderConfig>(&yaml).expect("round trip"),
        configured
    );
    configured.runpod.push(runpod_profile("saved"));
    assert_eq!(
        configured.validate(),
        Err(RemoteProviderConfigError::DuplicateRunPodProfile { index: 1 })
    );
}

#[test]
fn runpod_names_and_malformed_fields_fail_without_value_leakage() {
    for name in [
        String::new(),
        " private-marker".into(),
        "private-marker\0".into(),
        "a".repeat(192),
    ] {
        let configured = RemoteProviderConfig {
            runpod: vec![runpod_profile(&name)],
            ..Default::default()
        };
        let error = configured.validate().expect_err("invalid name");
        assert_eq!(error, RemoteProviderConfigError::InvalidRunPodProfile { index: 0 });
        assert!(!format!("{error:?} {error}").contains("private-marker"));
    }
    for field in ["api_key", "token", "unknown"] {
        let mut value = serde_json::to_value(runpod_profile("saved")).expect("serialize");
        value[field] = serde_json::json!("private-marker");
        let error = serde_json::from_value::<RemoteProviderConfig>(serde_json::json!({"runpod": [value]}))
            .expect_err("unknown profile field");
        assert!(!format!("{error:?} {error}").contains("private-marker"));
    }
    let error = serde_yaml::from_str::<RemoteProviderConfig>("runpod: [private-marker]").expect_err("shape");
    assert!(!error.to_string().contains("private-marker"));
}

fn profile(name: &str, docker_host: &str) -> LocalDockerProfile {
    LocalDockerProfile {
        name: name.into(),
        docker_host: docker_host.into(),
    }
}

fn config(profiles: Vec<LocalDockerProfile>) -> RemoteProviderConfig {
    RemoteProviderConfig {
        local_docker: profiles,
        ..Default::default()
    }
}

#[test]
fn empty_configuration_has_no_implicit_profile() {
    let empty = RemoteProviderConfig::default();
    assert!(empty.is_empty());
    assert_eq!(empty.validate(), Ok(()));
    assert_eq!(
        serde_yaml::from_str::<RemoteProviderConfig>("{}").expect("config"),
        empty
    );
    for name in ["", "default", "local", "DOCKER_HOST", "DOCKER_CONTEXT"] {
        assert_eq!(
            empty.local_docker_profile(name),
            Err(RemoteProviderConfigError::UnconfiguredLocalProfile)
        );
    }
    assert_eq!(serde_yaml::to_string(&empty).expect("serialize"), "{}\n");
}

#[test]
fn explicit_local_endpoints_round_trip_without_default_selection() {
    let expected = config(vec![
        profile("Desktop", "unix:///explicit path/not-connected.sock"),
        profile("Named pipe", "npipe:////./pipe/explicit-daemon"),
    ]);
    assert!(!expected.is_empty());
    assert_eq!(expected.validate(), Ok(()));
    let yaml = serde_yaml::to_string(&expected).expect("serialize");
    let restored: RemoteProviderConfig = serde_yaml::from_str(&yaml).expect("parse");
    assert_eq!(restored, expected);
    for configured in &expected.local_docker {
        assert_eq!(restored.local_docker_profile(&configured.name), Ok(configured));
    }
    assert_eq!(
        restored.local_docker_profile(""),
        Err(RemoteProviderConfigError::UnconfiguredLocalProfile)
    );
}

#[test]
fn profile_names_match_target_validation_and_lookup_is_exact() {
    for name in [
        String::new(),
        " ".into(),
        " local".into(),
        "local ".into(),
        "local\nname".into(),
        "local\0name".into(),
        "a".repeat(192),
        "ø".repeat(96),
    ] {
        let invalid = profile(&name, "unix:///explicit.sock");
        assert!(!valid_worker_profile_name(&name));
        assert_eq!(invalid.validate(), Err(LocalDockerError::InvalidTarget));
        assert_eq!(
            config(vec![invalid]).validate(),
            Err(RemoteProviderConfigError::InvalidLocalProfile { index: 0 })
        );
    }
    for name in ["a".repeat(191), "ø".repeat(95), "Team machine".into()] {
        assert!(valid_worker_profile_name(&name));
        assert_eq!(profile(&name, "unix:///explicit.sock").validate(), Ok(()));
    }
    let exact = config(vec![profile("Local", "unix:///explicit.sock")]);
    for name in ["local", "LOCAL", " Local", "Local "] {
        assert_eq!(
            exact.local_docker_profile(name),
            Err(RemoteProviderConfigError::UnconfiguredLocalProfile)
        );
    }
    let distinct = config(vec![
        profile("Local", "unix:///one.sock"),
        profile("local", "unix:///two.sock"),
    ]);
    assert_eq!(distinct.validate(), Ok(()));
    assert_eq!(distinct.local_docker_profile("local"), Ok(&distinct.local_docker[1]));
}

#[test]
fn duplicate_names_cannot_select_a_different_endpoint() {
    let ambiguous = config(vec![
        profile("local", "unix:///one.sock"),
        profile("local", "unix:///two.sock"),
    ]);
    assert_eq!(
        ambiguous.validate(),
        Err(RemoteProviderConfigError::DuplicateLocalProfile { index: 1 })
    );
    assert_eq!(
        ambiguous.local_docker_profile("local"),
        Err(RemoteProviderConfigError::DuplicateLocalProfile { index: 1 })
    );
}

#[test]
fn ambient_remote_and_malformed_endpoints_are_rejected() {
    for host in [
        "",
        "default",
        "/var/run/docker.sock",
        "unix://relative.sock",
        "unix:///",
        "unix:///path\n/socket",
        "npipe:////./pipe/",
        "npipe:////./pipe/name\0",
        "tcp://127.0.0.1:2375",
        "ssh://example.invalid",
        "https://example.invalid",
    ] {
        let invalid = profile("local", host);
        assert_eq!(invalid.validate(), Err(LocalDockerError::NonLocalDockerHost));
        let candidate = config(vec![profile("first", "unix:///first.sock"), invalid]);
        assert_eq!(
            candidate.validate(),
            Err(RemoteProviderConfigError::InvalidLocalProfile { index: 1 })
        );
        assert_eq!(
            candidate.local_docker_profile("first"),
            Err(RemoteProviderConfigError::InvalidLocalProfile { index: 1 })
        );
    }
}

#[test]
fn new_config_blocks_reject_unknown_or_missing_fields() {
    for yaml in [
        "implicit: true",
        "local_docker: [{name: local}]",
        "local_docker: [{docker_host: 'unix:///explicit.sock'}]",
        "local_docker: [{name: local, docker_host: 'unix:///explicit.sock', api_key: synthetic-private-marker}]",
    ] {
        let error = serde_yaml::from_str::<RemoteProviderConfig>(yaml).expect_err("invalid schema");
        assert!(!error.to_string().contains("synthetic-private-marker"));
    }
}

#[test]
fn malformed_profile_shapes_do_not_echo_values() {
    for yaml in [
        "local_docker: synthetic-private-marker",
        "local_docker: [synthetic-private-marker]",
        "local_docker: [{name: local, docker_host: [synthetic-private-marker]}]",
        "synthetic-private-marker: true",
    ] {
        let error = serde_yaml::from_str::<RemoteProviderConfig>(yaml).expect_err("invalid shape");
        assert!(!error.to_string().contains("synthetic-private-marker"));
    }
}

#[test]
fn configuration_errors_do_not_echo_profile_or_endpoint_values() {
    let marker = "synthetic-private-marker";
    let invalid = config(vec![profile(marker, &format!("ssh://{marker}"))]);
    let error = invalid.validate().expect_err("invalid endpoint");
    assert!(!format!("{error:?}: {error}").contains(marker));
    let duplicates = config(vec![
        profile(marker, "unix:///one.sock"),
        profile(marker, "unix:///two.sock"),
    ]);
    let error = duplicates.validate().expect_err("duplicate");
    assert!(!format!("{error:?}: {error}").contains(marker));
    let empty = RemoteProviderConfig::default();
    let error = empty.local_docker_profile(marker).expect_err("missing profile");
    assert!(!format!("{error:?}: {error}").contains(marker));
}

#[test]
fn provider_constructor_uses_same_validation_without_connecting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = CloudWorkflowStore::open_path(temp.path().join("private/store.sqlite3")).expect("store");
    assert!(matches!(
        LocalDockerInteractiveWorkerProvider::new(profile(" local", "unix:///explicit.sock"), store.clone()),
        Err(LocalDockerError::InvalidTarget)
    ));
    assert!(matches!(
        LocalDockerInteractiveWorkerProvider::new(profile("local", "ssh://example.invalid"), store.clone()),
        Err(LocalDockerError::NonLocalDockerHost)
    ));
    assert!(
        LocalDockerInteractiveWorkerProvider::new(profile("local", "unix:///not-created/not-connected.sock"), store)
            .is_ok()
    );
}
