use super::{
    Journal,
    provision::{allowed, first_fit, fit, location, plan},
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
fn a_volume_fixes_its_location_only_while_the_settings_allow_it() {
    let mut spec = spec();
    assert_eq!(location(Some("hel1"), &spec).unwrap(), "hel1");
    assert!(
        location(None, &spec).is_err(),
        "an existing volume always has a recorded location"
    );
    spec.data_centers = vec!["nbg1".into()];
    assert!(
        location(Some("hel1"), &spec).is_err(),
        "the volume cannot follow a changed allow-list"
    );
}

#[test]
fn without_a_volume_the_first_allowed_location_that_fits_is_chosen() {
    let mut spec = spec();
    spec.data_centers = vec!["fsn1".into(), "nbg1".into(), "hel1".into()];
    let offers = [
        offer("cx23", "fsn1", 2, 4.0),
        offer("cx33", "nbg1", 4, 8.0),
        offer("cx33", "hel1", 4, 8.0),
        offer("cx33", "ash", 4, 8.0),
    ];
    let (location, placements) = first_fit(&offers, &spec).unwrap();
    assert_eq!(location, "nbg1", "fsn1 has no fitting type, and ash is not allowed");
    assert_eq!(placements.len(), 1);
    spec.data_centers = vec!["ash".into()];
    assert!(first_fit(&offers[..3], &spec).is_err());
    spec.data_centers.clear();
    assert!(first_fit(&offers, &spec).is_err());
}

#[test]
fn a_placed_server_is_reconciled_against_the_settings_not_the_catalog() {
    let spec = spec();
    let placements = allowed(&spec, "hel1");
    let types: Vec<_> = placements.iter().map(|p| p.server_type.as_str()).collect();
    assert_eq!(types, ["cx23", "cx33", "cpx32"]);
    assert!(placements.iter().all(|p| p.location == "hel1"));
}

#[test]
fn a_malformed_worker_key_is_refused_by_the_spec() {
    let mut spec = spec();
    spec.validate().unwrap();
    spec.public_key = String::new();
    assert!(
        spec.validate().is_err(),
        "an empty key would boot a worker nobody can reach"
    );
}

#[test]
fn readiness_accepts_only_a_server_and_volume_that_hold_each_other() {
    use super::readiness::holds;
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

/// A scripted Hetzner API. A response containing `@PUBLIC_KEY@` echoes the
/// public key of the request it answers, as Hetzner does for a new SSH key.
#[cfg(unix)]
mod provider {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    pub(super) type Requests = Arc<Mutex<Vec<String>>>;

    pub(super) fn serve(responses: Vec<(u16, String)>) -> (SocketAddr, Requests, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let task = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for (status, body) in responses {
                // A request the script expects but provisioning never sends ends the fake.
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if std::time::Instant::now() > deadline {
                                return;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("fake accept failed: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut input = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    input.extend_from_slice(&buffer[..read]);
                    if let Some(end) = input.windows(4).position(|window| window == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&input[..end]).to_ascii_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if input.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(input).unwrap();
                let body = if body.contains("@PUBLIC_KEY@") {
                    let sent: serde_json::Value =
                        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                    body.replace("@PUBLIC_KEY@", sent["public_key"].as_str().unwrap())
                } else {
                    body
                };
                observed.lock().unwrap().push(request);
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (address, requests, task)
    }
}

#[cfg(unix)]
mod failure_points {
    use super::{provider, spec, volume};
    use crate::cloud_runtime::{
        deployment::hetzner::{Compute, Journal, provision, retained},
        state::{Deployment, Store},
    };
    use horizon_cloud::{Cancellation, CreateState, Credential, hetzner::Hetzner};
    use serde_json::{Value, json};

    fn listing(key: &str, items: &Value) -> String {
        let mut page = json!({"meta": {"pagination": {"next_page": null}}});
        page[key] = items.clone();
        page.to_string()
    }
    fn error(status: u16, code: &str) -> (u16, String) {
        (status, json!({"error": {"code": code, "message": code}}).to_string())
    }
    fn server_types(cores: u32) -> String {
        let price = json!({"location": "hel1", "price_hourly": {"net": "0.01"}, "price_monthly": {"net": "5"}});
        let kind = json!({"name": "cx33", "cores": cores, "memory": 8.0, "disk": 80, "cpu_type": "shared",
            "architecture": "x86", "prices": [price],
            "locations": [{"name": "hel1", "available": true, "recommended": true, "deprecation": null}]});
        listing("server_types", &json!([kind]))
    }
    fn locations() -> String {
        listing("locations", &json!([{"name": "hel1", "network_zone": "eu-central"}]))
    }
    fn pricing() -> String {
        json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.05"}}, "primary_ips": []}})
            .to_string()
    }
    fn key() -> String {
        json!({"ssh_key": {"id": 5, "name": format!("horizon-cloud-{}", spec().operation_id), "public_key": "@PUBLIC_KEY@",
            "labels": {"horizon-operation": spec().operation_id}}})
        .to_string()
    }
    fn created_volume() -> String {
        let mut value = serde_json::to_value(volume()).unwrap();
        value["server"] = Value::Null;
        json!({"volume": value, "action": {"id": 1, "status": "success"}}).to_string()
    }
    fn free_volume() -> String {
        let mut value = serde_json::to_value(volume()).unwrap();
        value["server"] = Value::Null;
        json!({"volume": value}).to_string()
    }
    fn created_server(cores: u32) -> String {
        json!({"server": {"id": 42, "name": format!("horizon-cloud-{}", spec().operation_id), "status": "initializing",
            "public_net": {"ipv4": null}, "server_type": {"name": "cx33", "cores": cores, "memory": 8.0, "disk": 80},
            "location": {"name": "hel1"}, "labels": {"horizon-operation": spec().operation_id}, "volumes": [9]},
            "action": {"id": 2, "status": "success"}, "next_actions": []})
        .to_string()
    }

    /// Runs provisioning for a fresh cloud against `responses` and returns the
    /// saved deployment, the journal and the number of requests served.
    fn provision_with(responses: Vec<(u16, String)>) -> (Deployment, Journal, bool, usize) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let spec = spec();
        let mut state: Deployment = serde_json::from_value(json!({
            "version": 1, "cloud_id": spec.operation_id, "repository": "/fixture", "revision": "a".repeat(40),
            "profile": spec.profile, "stage": "Provision", "operation": {"state": "prepared"}, "spec": spec,
            "worker": null, "sessions": []
        }))
        .unwrap();
        store.save(&state).unwrap();
        let (address, requests, task) = provider::serve(responses);
        let compute = Compute {
            client: Hetzner::loopback(Credential::new("secret-test-key".into()).unwrap(), address).unwrap(),
            settings: serde_json::from_value(
                json!({"token_file": "/unused", "server_types": ["cx33"], "locations": ["hel1"]}),
            )
            .unwrap(),
        };
        let result = provision(&compute, &store, &mut state, &spec, &Cancellation::default(), &|_| {});
        task.join().unwrap();
        let served = requests.lock().unwrap().len();
        assert!(result.is_err() || served > 0);
        let saved = store.load().unwrap().unwrap();
        let journal = Journal::load(root.path()).unwrap();
        let kept = retained(root.path()).unwrap();
        (saved, journal, kept, served)
    }

    #[test]
    fn nothing_is_created_when_no_type_fits() {
        let (state, journal, kept, served) =
            provision_with(vec![(200, server_types(2)), (200, locations()), (200, pricing())]);
        assert_eq!(served, 3, "only the catalog was read");
        assert!(journal.key.is_none() && journal.location.is_none() && !kept);
        assert_eq!(state.operation, CreateState::Prepared);
    }

    #[test]
    fn a_failed_key_registration_leaves_the_key_recorded_for_deletion() {
        let (state, journal, kept, _) = provision_with(vec![
            (200, server_types(4)),
            (200, locations()),
            (200, pricing()),
            (200, listing("ssh_keys", &json!([]))),
            error(503, "unavailable"),
        ]);
        assert!(
            journal.key.is_some() && kept,
            "a key may exist on Hetzner, so the cloud keeps it"
        );
        assert!(
            journal.location.is_none(),
            "no volume was requested, so no location is fixed"
        );
        assert_eq!(
            (journal.volume, state.operation),
            (CreateState::Prepared, CreateState::Prepared)
        );
    }

    #[test]
    fn an_uncertain_volume_request_stays_fenced_for_reconciliation() {
        let (state, journal, kept, _) = provision_with(vec![
            (200, server_types(4)),
            (200, locations()),
            (200, pricing()),
            (200, listing("ssh_keys", &json!([]))),
            (201, key()),
            (200, listing("volumes", &json!([]))),
            error(503, "unavailable"),
        ]);
        assert_eq!(journal.volume, CreateState::Requested);
        assert_eq!(
            journal.location.as_deref(),
            Some("hel1"),
            "recorded before the request it fixes"
        );
        assert!(kept);
        assert_eq!(state.operation, CreateState::Prepared);
    }

    #[test]
    fn an_uncertain_server_request_stays_fenced_with_its_volume_bound() {
        let (state, journal, kept, _) = provision_with(vec![
            (200, server_types(4)),
            (200, locations()),
            (200, pricing()),
            (200, listing("ssh_keys", &json!([]))),
            (201, key()),
            (200, listing("volumes", &json!([]))),
            (201, created_volume()),
            (200, free_volume()),
            (200, listing("servers", &json!([]))),
            (200, free_volume()),
            error(503, "unavailable"),
        ]);
        assert_eq!(journal.volume, CreateState::Bound { worker_id: "9".into() });
        assert_eq!(state.operation, CreateState::Requested);
        assert!(kept && state.worker.is_none());
    }

    #[test]
    fn a_server_below_the_profile_is_bound_but_never_recorded_as_the_worker() {
        let (state, journal, kept, _) = provision_with(vec![
            (200, server_types(4)),
            (200, locations()),
            (200, pricing()),
            (200, listing("ssh_keys", &json!([]))),
            (201, key()),
            (200, listing("volumes", &json!([]))),
            (201, created_volume()),
            (200, free_volume()),
            (200, listing("servers", &json!([]))),
            (200, free_volume()),
            (201, created_server(2)),
        ]);
        assert_eq!(
            state.operation,
            CreateState::Bound { worker_id: "42".into() },
            "bound so it can be deleted"
        );
        assert!(
            state.worker.is_none(),
            "an unverified server is never recorded as the worker"
        );
        assert!(kept && matches!(journal.volume, CreateState::Bound { .. }));
    }

    #[test]
    fn a_verified_server_becomes_the_worker() {
        let (state, _, _, _) = provision_with(vec![
            (200, server_types(4)),
            (200, locations()),
            (200, pricing()),
            (200, listing("ssh_keys", &json!([]))),
            (201, key()),
            (200, listing("volumes", &json!([]))),
            (201, created_volume()),
            (200, free_volume()),
            (200, listing("servers", &json!([]))),
            (200, free_volume()),
            (201, created_server(4)),
        ]);
        assert_eq!(state.worker.unwrap().id, "42");
    }
}
