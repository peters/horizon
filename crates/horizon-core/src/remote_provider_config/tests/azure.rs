use super::*;
use crate::{Config, cloud_run::azure::AzureDiskSku};

const SUBSCRIPTION: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";
const MARKER: &str = "synthetic-private-marker";

fn cpu_profile(name: &str) -> AzureProfile {
    AzureProfile {
        name: name.into(),
        subscription_id: SUBSCRIPTION.into(),
        location: "northeurope".into(),
        vm_size: "Standard_D2s_v3".into(),
        image_pull_identity_id: format!(
            "/subscriptions/{SUBSCRIPTION}/resourceGroups/synthetic-registry/providers/Microsoft.ManagedIdentity/userAssignedIdentities/puller"
        ),
        declared_hourly_cost_micros: 100_000,
        registry_login_server: "example.azurecr.io".into(),
        disk_sku: AzureDiskSku::default(),
    }
}

fn configured() -> RemoteProviderConfig {
    RemoteProviderConfig {
        azure: vec![cpu_profile("saved")],
        ..Default::default()
    }
}

#[test]
fn cpu_profiles_are_explicit_case_sensitive_and_provider_scoped() {
    let mut remote = configured();
    remote.local_docker.push(profile("saved", "unix:///unused.sock"));
    remote.runpod.push(runpod_profile("saved"));
    assert_eq!(remote.validate(), Ok(()));
    assert_eq!(remote.azure_profile("saved"), Ok(&remote.azure[0]));
    for name in ["", "Saved", " saved", "saved ", "default", "AZURE_SUBSCRIPTION_ID"] {
        assert_eq!(
            remote.azure_profile(name),
            Err(RemoteProviderConfigError::UnconfiguredAzureProfile)
        );
    }
    remote.azure.push(cpu_profile("Saved"));
    assert_eq!(remote.azure_profile("Saved"), Ok(&remote.azure[1]));
    remote.azure.push(cpu_profile("saved"));
    assert_eq!(
        remote.validate(),
        Err(RemoteProviderConfigError::DuplicateAzureProfile { index: 2 })
    );
    assert_eq!(
        remote.azure_profile("saved"),
        Err(RemoteProviderConfigError::DuplicateAzureProfile { index: 2 })
    );
}

#[test]
fn empty_and_legacy_configuration_never_selects_a_cpu_profile() {
    for yaml in ["{}", "remote: {}", "remote: {azure: []}"] {
        let config = Config::from_yaml(yaml).expect("legacy or empty configuration");
        assert!(config.remote.azure.is_empty() && config.remote.is_empty());
        assert_eq!(
            config.remote.azure_profile("saved"),
            Err(RemoteProviderConfigError::UnconfiguredAzureProfile)
        );
        assert!(!config.to_yaml().expect("serialize").contains("\nremote:"));
    }
    for yaml in [
        "local_docker: [{name: local, docker_host: 'unix:///unused.sock'}]",
        "runpod: []",
    ] {
        let remote: RemoteProviderConfig = serde_yaml::from_str(yaml).expect("existing profile schema");
        assert!(remote.azure.is_empty());
        assert!(!serde_yaml::to_string(&remote).expect("serialize").contains("azure:"));
    }
}

#[test]
fn cpu_profile_round_trips_through_full_config_without_credential_fields() {
    for disk_sku in [AzureDiskSku::StandardSsdLrs, AzureDiskSku::PremiumLrs] {
        let mut config = Config {
            remote: configured(),
            ..Default::default()
        };
        config.remote.azure[0].disk_sku = disk_sku;
        let yaml = config.to_yaml().expect("serialize");
        let restored = Config::from_yaml(&yaml).expect("full configuration");
        assert_eq!(restored.remote, config.remote);
        assert!(!restored.remote.is_empty());
        for credential in ["access_token:", "client_secret:", "api_key:", "password:"] {
            assert!(!yaml.contains(credential));
        }
    }
    let mut value = serde_json::to_value(configured()).expect("serialize");
    value["azure"][0].as_object_mut().expect("profile").remove("disk_sku");
    assert_eq!(
        serde_json::from_value::<RemoteProviderConfig>(value).expect("default disk SKU"),
        configured()
    );
}

#[test]
fn invalid_cpu_placement_fails_before_lookup_without_value_leakage() {
    let mutations: [fn(&mut AzureProfile); 7] = [
        |p| p.name = format!(" {MARKER}"),
        |p| p.subscription_id = MARKER.into(),
        |p| p.location = MARKER.into(),
        |p| p.vm_size = MARKER.into(),
        |p| p.image_pull_identity_id = MARKER.into(),
        |p| p.registry_login_server = MARKER.into(),
        |p| p.declared_hourly_cost_micros = 0,
    ];
    for mutate in mutations {
        let mut remote = configured();
        remote.azure.push(cpu_profile("other"));
        mutate(&mut remote.azure[1]);
        let expected = Err(RemoteProviderConfigError::InvalidAzureProfile { index: 1 });
        assert_eq!(remote.validate(), expected);
        let error = remote.azure_profile("saved").expect_err("invalid unrelated profile");
        assert_eq!(error, RemoteProviderConfigError::InvalidAzureProfile { index: 1 });
        assert!(!format!("{error:?}: {error}").contains(MARKER));
        let config = Config {
            remote,
            ..Default::default()
        };
        let error =
            Config::from_yaml(&config.to_yaml().expect("serialize malformed values")).expect_err("semantic validation");
        assert!(!format!("{error:?}: {error}").contains(MARKER));
    }
}

#[test]
fn malformed_cpu_profiles_and_secret_fields_are_redacted() {
    for field in ["access_token", "client_secret", "api_key", "password", "unknown"] {
        let mut value = serde_json::to_value(configured()).expect("serialize");
        value["azure"][0][field] = serde_json::json!(MARKER);
        let yaml = serde_yaml::to_string(&serde_json::json!({"remote": value})).expect("serialize");
        let error = Config::from_yaml(&yaml).expect_err("unknown credential field");
        assert!(!format!("{error:?}: {error}").contains(MARKER));
    }
    for value in [
        serde_json::json!(MARKER),
        serde_json::json!([MARKER]),
        serde_json::json!([{"name": MARKER}]),
        serde_json::json!([{"name": [MARKER]}]),
    ] {
        let yaml = serde_yaml::to_string(&serde_json::json!({"remote": {"azure": value}})).expect("serialize");
        let error = Config::from_yaml(&yaml).expect_err("malformed profile");
        assert!(!format!("{error:?}: {error}").contains(MARKER));
    }
    let mut value = serde_json::to_value(configured()).expect("serialize");
    value["azure"][0]["disk_sku"] = serde_json::json!(MARKER);
    let error = serde_json::from_value::<RemoteProviderConfig>(value).expect_err("invalid disk SKU");
    assert!(!format!("{error:?}: {error}").contains(MARKER));
}

#[test]
fn cpu_configuration_load_preserves_file_without_creating_runtime_state() {
    let directory = tempfile::tempdir().expect("temporary config home");
    let path = directory.path().join("config.yaml");
    let config = Config {
        remote: configured(),
        ..Default::default()
    };
    let original = config.to_yaml().expect("serialize");
    std::fs::write(&path, &original).expect("write fixture");
    assert_eq!(Config::load(Some(&path)).expect("load").remote, config.remote);
    assert_eq!(std::fs::read_to_string(&path).expect("retained config"), original);
    assert_eq!(std::fs::read_dir(directory.path()).expect("directory").count(), 1);
}
