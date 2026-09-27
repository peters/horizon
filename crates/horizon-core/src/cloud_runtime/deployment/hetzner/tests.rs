use super::{
    Journal,
    provision::{fit, location, plan},
    throwaway_public_key, worker,
};
use horizon_cloud::{
    CreateState, WorkerSpec,
    hetzner::{catalog::Offer, servers::Server, volumes::Volume},
};

fn spec() -> WorkerSpec {
    let profile: horizon_cloud::Profile = serde_json::from_value(serde_json::json!({
        "provider": "hetzner", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
        "storage": {"container_gb": 20, "volume_gb": 50}
    }))
    .unwrap();
    WorkerSpec {
        operation_id: "0e9f3c52-8f55-4a4c-9d7c-1c1c0a6a7b21".into(),
        image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
        profile,
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".into(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: vec!["cx23".into(), "cx33".into(), "cpx32".into()],
        data_centers: vec!["hel1".into()],
        startup_metadata: None,
    }
}

fn server(status: &str, address: Option<&str>) -> Server {
    serde_json::from_value(serde_json::json!({
        "id": 42, "name": format!("horizon-cloud-{}", spec().operation_id), "status": status,
        "public_net": {"ipv4": address.map(|ip| serde_json::json!({"ip": ip}))},
        "server_type": {"name": "cx33", "cores": 4, "memory": 8.0, "disk": 80},
        "location": {"name": "hel1"}, "labels": {"horizon-operation": spec().operation_id}, "volumes": [9]
    }))
    .unwrap()
}

fn volume() -> Volume {
    serde_json::from_value(serde_json::json!({
        "id": 9, "name": format!("horizon-cloud-{}", spec().operation_id), "size": 50, "location": {"name": "hel1"},
        "server": 42, "linux_device": "/dev/disk/by-id/scsi-0HC_Volume_9", "status": "available",
        "labels": {"horizon-operation": spec().operation_id}
    }))
    .unwrap()
}

fn offer(server_type: &str, location: &str, cores: u32, memory_gb: f64) -> Offer {
    Offer {
        server_type: server_type.into(),
        location: location.into(),
        cores,
        memory_gb,
        disk_gb: 80,
        dedicated: false,
        hourly_eur: 0.01,
        monthly_eur: 5.0,
        available: false,
        recommended: false,
    }
}

#[test]
fn the_journal_starts_prepared_and_round_trips_durably() {
    let root = tempfile::tempdir().unwrap();
    let fresh = Journal::load(root.path()).unwrap();
    assert_eq!(fresh.volume, CreateState::Prepared);
    assert!(fresh.location.is_none() && fresh.key.is_none());
    let saved = Journal {
        location: Some("hel1".into()),
        volume: CreateState::Bound { worker_id: "9".into() },
        key: Some(throwaway_public_key().unwrap()),
    };
    saved.save(root.path()).unwrap();
    assert_eq!(Journal::load(root.path()).unwrap(), saved);
    std::fs::write(
        root.path().join("hetzner.json"),
        br#"{"volume":{"state":"prepared"},"extra":1}"#,
    )
    .unwrap();
    assert!(
        Journal::load(root.path()).is_err(),
        "an unknown field is refused, not dropped"
    );
}

#[test]
fn a_running_server_is_described_as_a_verified_worker() {
    let spec = spec();
    let running = worker(&server("running", Some("192.0.2.10")), &spec, &volume()).unwrap();
    running.verify(&spec).unwrap();
    assert_eq!(running.ssh_address().unwrap().to_string(), "192.0.2.10:22");
    assert_eq!(running.status(), horizon_cloud::WorkerStatus::Running);
    assert_eq!((running.vcpu_count, running.memory_in_gb), (Some(4), Some(8)));
    assert_eq!(running.data_center(), Some("hel1"));
    running.verify_resources(&spec).unwrap();
    let mut larger = spec.clone();
    larger.profile.storage.container_gb = 100;
    assert!(
        running.verify_resources(&larger).is_err(),
        "an 80 GB local disk cannot hold a 100 GB container disk"
    );
    let off = worker(&server("off", None), &spec, &volume()).unwrap();
    assert_eq!(off.status(), horizon_cloud::WorkerStatus::Stopped);
    assert!(off.ssh_address().is_none());
}

#[test]
fn the_throwaway_key_is_a_valid_distinct_ed25519_key() {
    let first = throwaway_public_key().unwrap();
    assert!(horizon_cloud::valid_public_key(&first), "{first}");
    assert_ne!(first, throwaway_public_key().unwrap());
}

#[test]
fn unsupported_requests_fail_the_preflight_before_any_state() {
    use super::preflight;
    let root = tempfile::tempdir().unwrap();
    let token = root.path().join("token");
    std::fs::write(&token, "secret-token").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut settings: crate::cloud_runtime::settings::Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": "/unused", "docker_config": "/unused",
        "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": [],
        "hetzner": {"token_file": token, "server_types": ["cx23"], "locations": ["hel1"]}
    }))
    .unwrap();
    let mut spec = spec();
    preflight(&spec.operation_id, &spec.profile, &settings).unwrap();
    assert!(
        preflight("Cloud_1", &spec.profile, &settings).is_err(),
        "not a Hetzner resource name"
    );
    spec.profile.idle_stop_minutes = Some(30);
    assert!(preflight(&spec.operation_id, &spec.profile, &settings).is_err());
    spec.profile.idle_stop_minutes = None;
    let mut private = settings.clone();
    private.registries = serde_json::from_value(serde_json::json!({"root": root.path(), "bindings": [{
        "repository": "registry.example/worker", "generation": "generation1", "read_only_confirmed": true,
        "pull": {"username": "reader", "secret_file": root.path().join("pull"), "expires_at": null}
    }]}))
    .unwrap();
    assert_eq!(
        preflight(&spec.operation_id, &spec.profile, &private)
            .unwrap_err()
            .to_string(),
        "This image is private; add hetzner.registry_pull with a read-only pull token before deploying on Hetzner"
    );
    let password = root.path().join("pull");
    std::fs::write(&password, "pull-secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let login = |server: &str| crate::cloud_runtime::settings::RegistryPull {
        server: server.into(),
        username: "pull".into(),
        password_file: password.clone(),
    };
    private.hetzner.as_mut().unwrap().registry_pull = Some(login("registry.example"));
    preflight(&spec.operation_id, &spec.profile, &private).unwrap();
    private.hetzner.as_mut().unwrap().registry_pull = Some(login("other.example"));
    assert_eq!(
        preflight(&spec.operation_id, &spec.profile, &private)
            .unwrap_err()
            .to_string(),
        "hetzner.registry_pull names a different registry than the profile's image"
    );
    let mut hub = spec.profile.clone();
    hub.image = "team/worker".into();
    settings.hetzner.as_mut().unwrap().registry_pull = Some(login("index.docker.io"));
    preflight(&spec.operation_id, &hub, &settings).unwrap();
    settings.hetzner.as_mut().unwrap().registry_pull = Some(crate::cloud_runtime::settings::RegistryPull {
        server: "example.azurecr.io".into(),
        username: "pull".into(),
        password_file: root.path().join("missing"),
    });
    assert!(
        preflight(&spec.operation_id, &spec.profile, &settings).is_err(),
        "an unreadable pull credential"
    );
    settings.hetzner = None;
    assert!(
        preflight(&spec.operation_id, &spec.profile, &settings).is_err(),
        "no Hetzner settings"
    );
}

#[test]
fn only_configured_types_with_enough_cpu_and_memory_in_the_location_are_tried() {
    let spec = spec();
    let offers = [
        offer("cx23", "hel1", 2, 4.0),
        offer("cx33", "hel1", 4, 8.0),
        offer("cpx32", "hel1", 4, 8.0),
        offer("cpx32", "nbg1", 4, 8.0),
        offer("ccx23", "hel1", 4, 16.0),
    ];
    let placements = fit(&offers, &spec, "hel1").unwrap();
    let chosen: Vec<_> = placements
        .iter()
        .map(|p| (p.server_type.as_str(), p.location.as_str()))
        .collect();
    assert_eq!(
        chosen,
        [("cx33", "hel1"), ("cpx32", "hel1")],
        "in configured order, unlisted types ignored"
    );
    assert!(fit(&offers[..1], &spec, "hel1").is_err());
    let mut small_disk = spec.clone();
    small_disk.profile.storage.container_gb = 100;
    assert!(
        fit(&offers, &small_disk, "hel1").is_err(),
        "an 80 GB local disk cannot hold a 100 GB container disk"
    );
    assert!(fit(&offers, &spec, "fsn1").is_err());
}

#[test]
fn the_host_plan_carries_the_worker_contract_and_no_idle_stop() {
    let spec = spec();
    let rendered = plan(&spec, "/dev/disk/by-id/scsi-0HC_Volume_9", None)
        .unwrap()
        .cloud_config()
        .unwrap();
    assert!(rendered.starts_with("#cloud-config\n"));
    for expected in [
        "PUBLIC_KEY=ssh-ed25519 ",
        &format!("HORIZON_CLOUD_OPERATION={}", spec.operation_id),
        "HORIZON_WORKER_CAPABILITIES=",
        "/dev/disk/by-id/scsi-0HC_Volume_9",
        &spec.image_digest,
    ] {
        assert!(rendered.contains(expected), "{expected}");
    }
    assert!(!rendered.contains("HORIZON_IDLE_STOP_MINUTES"));
}

#[test]
fn a_recorded_location_is_kept_only_while_the_settings_allow_it() {
    let mut spec = spec();
    assert_eq!(location(None, &spec).unwrap(), "hel1");
    assert_eq!(location(Some("hel1"), &spec).unwrap(), "hel1");
    spec.data_centers = vec!["nbg1".into()];
    assert!(
        location(Some("hel1"), &spec).is_err(),
        "the volume cannot follow a changed allow-list"
    );
    spec.data_centers.clear();
    assert!(location(None, &spec).is_err());
}
