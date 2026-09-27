use super::*;

mod lifecycle;
mod sequence;
use crate::hetzner::{catalog::Offer, servers::Server, volumes::Volume};

fn spec() -> WorkerSpec {
    let profile: crate::Profile = serde_json::from_value(serde_json::json!({
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
fn a_running_server_is_described_as_a_verified_worker() {
    let spec = spec();
    let running = worker(&server("running", Some("192.0.2.10")), &spec, &volume()).unwrap();
    running.verify(&spec).unwrap();
    assert_eq!(running.ssh_address().unwrap().to_string(), "192.0.2.10:22");
    assert_eq!(running.status(), crate::WorkerStatus::Running);
    assert_eq!((running.vcpu_count, running.memory_in_gb), (Some(4), Some(8)));
    assert_eq!(running.data_center(), Some("hel1"));
    running.verify_resources(&spec).unwrap();
    let mut larger = spec.clone();
    larger.profile.storage.container_gb = 100;
    // An 80 GB local disk cannot hold a 100 GB container disk.
    assert!(running.verify_resources(&larger).is_err());
    let off = worker(&server("off", None), &spec, &volume()).unwrap();
    assert_eq!(off.status(), crate::WorkerStatus::Stopped);
    let unknown = worker(&server("unknown", None), &spec, &volume()).unwrap();
    assert_eq!(unknown.status(), crate::WorkerStatus::Lost);
    assert!(off.ssh_address().is_none());
}

#[test]
fn a_worker_with_an_idle_period_is_described_with_it() {
    let mut spec = spec();
    spec.profile.idle_stop_minutes = Some(30);
    let described = worker(&server("running", Some("192.0.2.10")), &spec, &volume()).unwrap();
    // The host plan passes the period, so the worker verifies against its own spec only.
    described.verify(&spec).unwrap();
    spec.profile.idle_stop_minutes = Some(40);
    assert!(described.verify(&spec).is_err());
}

#[test]
fn the_throwaway_key_is_a_valid_distinct_ed25519_key() {
    let first = throwaway_public_key().unwrap();
    assert!(crate::valid_public_key(&first), "{first}");
    assert_ne!(first, throwaway_public_key().unwrap());
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
    let types = spec.cpu_flavors.clone();
    let placements = fit(&offers, &spec, &types, "hel1").unwrap();
    let chosen: Vec<_> = placements
        .iter()
        .map(|p| (p.server_type.as_str(), p.location.as_str()))
        .collect();
    assert_eq!(
        chosen,
        [("cx33", "hel1"), ("cpx32", "hel1")],
        "in configured order, unlisted types ignored"
    );
    assert!(fit(&offers[..1], &spec, &types, "hel1").is_err());
    let mut small_disk = spec.clone();
    small_disk.profile.storage.container_gb = 100;
    // An 80 GB local disk cannot hold a 100 GB container disk.
    assert!(fit(&offers, &small_disk, &types, "hel1").is_err());
    assert!(fit(&offers, &spec, &types, "fsn1").is_err());
}

#[test]
fn the_host_plan_carries_the_worker_contract_and_its_idle_period() {
    let mut spec = spec();
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
    spec.profile.idle_stop_minutes = Some(30);
    let rendered = plan(&spec, "/dev/disk/by-id/scsi-0HC_Volume_9", None)
        .unwrap()
        .cloud_config()
        .unwrap();
    assert!(rendered.contains("HORIZON_IDLE_STOP_MINUTES=30"));
}

fn allowing(locations: &[&str], server_types: &[&str]) -> Policy {
    Policy {
        locations: locations.iter().map(|&name| name.into()).collect(),
        server_types: server_types.iter().map(|&name| name.into()).collect(),
    }
}

#[test]
fn a_volume_fixes_its_location_only_while_the_settings_allow_it() {
    let allowed = allowing(&["hel1"], &["cx33"]);
    assert_eq!(location(Some("hel1"), &allowed).unwrap(), "hel1");
    // An existing volume always has a recorded location.
    assert!(location(None, &allowed).is_err());
    // The volume cannot follow a changed allow-list.
    assert!(location(Some("hel1"), &allowing(&["nbg1"], &["cx33"])).is_err());
}

#[test]
fn without_a_volume_the_first_allowed_location_that_fits_is_chosen() {
    let spec = spec();
    let offers = [
        offer("cx23", "fsn1", 2, 4.0),
        offer("cx33", "nbg1", 4, 8.0),
        offer("cx33", "hel1", 4, 8.0),
        offer("cx33", "ash", 4, 8.0),
    ];
    let (location, placements) =
        first_fit(&offers, &spec, &allowing(&["fsn1", "nbg1", "hel1"], &["cx23", "cx33"])).unwrap();
    assert_eq!(location, "nbg1", "fsn1 has no fitting type, and ash is not allowed");
    assert_eq!(placements.len(), 1);
    assert!(first_fit(&offers[..3], &spec, &allowing(&["ash"], &["cx33"])).is_err());
    assert!(first_fit(&offers, &spec, &allowing(&[], &["cx33"])).is_err());
    // Only the current allowed types are considered.
    assert!(first_fit(&offers, &spec, &allowing(&["nbg1"], &["cx23"])).is_err());
}

#[test]
fn a_placed_server_is_reconciled_against_the_current_types_not_the_catalog() {
    let placements = allowed(&["cx33".into(), "cpx32".into()], "hel1");
    let types: Vec<_> = placements.iter().map(|p| p.server_type.as_str()).collect();
    assert_eq!(types, ["cx33", "cpx32"]);
    assert!(placements.iter().all(|p| p.location == "hel1"));
}

#[test]
fn readiness_admits_only_a_server_the_current_policy_allows_where_its_volume_is() {
    let server = server("running", Some("192.0.2.10"));
    let allowed = allowing(&["hel1"], &["cx33"]);
    assert!(admitted(&server, &allowed, Some("hel1")));
    // A resized server type.
    assert!(!admitted(&server, &allowing(&["hel1"], &["cpx32"]), Some("hel1")));
    // A location no longer allowed.
    assert!(!admitted(&server, &allowing(&["nbg1"], &["cx33"]), Some("hel1")));
    assert!(!admitted(&server, &allowed, Some("nbg1")), "not where its volume is");
}

#[test]
fn a_malformed_worker_key_is_refused_by_the_spec() {
    let mut spec = spec();
    spec.validate().unwrap();
    spec.public_key = String::new();
    // An empty key would boot a worker nobody can reach.
    assert!(spec.validate().is_err());
}

#[test]
fn readiness_accepts_only_a_server_and_volume_that_hold_each_other() {
    let (server, volume) = (server("running", Some("192.0.2.10")), volume());
    assert!(holds(&server, &volume));
    let mut detached = volume.clone();
    detached.server = None;
    assert!(!holds(&server, &detached));
    let mut elsewhere = server.clone();
    elsewhere.location.name = "nbg1".into();
    assert!(!holds(&elsewhere, &volume));
    let mut extra = server.clone();
    extra.volumes = vec![9, 10];
    assert!(!holds(&extra, &volume));
}
