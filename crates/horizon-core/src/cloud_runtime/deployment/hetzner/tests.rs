use super::{
    Allowed, Journal,
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
        released: Some("42".into()),
    };
    saved.save(root.path()).unwrap();
    assert_eq!(Journal::load(root.path()).unwrap(), saved);
    std::fs::write(
        root.path().join("hetzner.json"),
        br#"{"volume":{"state":"prepared"},"extra":1}"#,
    )
    .unwrap();
    // An unknown field is refused, not dropped.
    assert!(Journal::load(root.path()).is_err());
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
    // An 80 GB local disk cannot hold a 100 GB container disk.
    assert!(running.verify_resources(&larger).is_err());
    let off = worker(&server("off", None), &spec, &volume()).unwrap();
    assert_eq!(off.status(), horizon_cloud::WorkerStatus::Stopped);
    let unknown = worker(&server("unknown", None), &spec, &volume()).unwrap();
    assert_eq!(unknown.status(), horizon_cloud::WorkerStatus::Lost);
    assert!(off.ssh_address().is_none());
}

#[test]
fn the_throwaway_key_is_a_valid_distinct_ed25519_key() {
    let first = throwaway_public_key().unwrap();
    assert!(horizon_cloud::valid_public_key(&first), "{first}");
    assert_ne!(first, throwaway_public_key().unwrap());
}

/// Writes `contents` to `path` readable only by its owner, as secret files must be.
fn private_file(path: &std::path::Path, contents: &str) -> std::path::PathBuf {
    std::fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path.to_path_buf()
}

#[test]
fn unsupported_requests_fail_the_preflight_before_any_state() {
    use super::{admit, preflight, pull_login};
    let root = tempfile::tempdir().unwrap();
    let token = private_file(&root.path().join("token"), "secret-token");
    let mut settings: crate::cloud_runtime::settings::Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": "/unused", "docker_config": "/unused",
        "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": [],
        "hetzner": {"token_file": token, "server_types": ["cx23"], "locations": ["hel1"]}
    }))
    .unwrap();
    let mut spec = spec();
    preflight(&spec.operation_id, &spec.profile, &settings).unwrap();
    // A cloud placed where the settings no longer allow can still be stopped and deleted.
    let mut elsewhere = settings.clone();
    elsewhere.placement = serde_json::from_value(serde_json::json!({"data_centers": ["nbg1"]})).unwrap();
    assert!(super::Compute::new(&elsewhere).is_err());
    super::Compute::cleanup(&elsewhere).unwrap();
    // Not a Hetzner resource name.
    assert!(preflight("Cloud_1", &spec.profile, &settings).is_err());
    spec.profile.idle_stop_minutes = Some(30);
    assert!(preflight(&spec.operation_id, &spec.profile, &settings).is_err());
    spec.profile.idle_stop_minutes = None;
    // The pull login is checked where provisioning loads it, before any request.
    let pull = |settings: &crate::cloud_runtime::settings::Settings, image: &str| {
        pull_login(settings.hetzner.as_ref().unwrap(), settings.registries.as_ref(), image).map(|login| login.is_some())
    };
    let mut private = settings.clone();
    private.registries = serde_json::from_value(serde_json::json!({"root": root.path(), "bindings": [{
        "repository": "registry.example/worker", "generation": "generation1", "read_only_confirmed": true,
        "pull": {"username": "reader", "secret_file": root.path().join("pull"), "expires_at": null}
    }]}))
    .unwrap();
    assert_eq!(
        pull(&private, &spec.profile.image).unwrap_err().to_string(),
        "This image is private; add hetzner.registry_pull with a read-only pull token before deploying on Hetzner"
    );
    // Checked before the deployment is recorded, unless its server is only reconciled.
    assert!(admit(&private, &spec.profile.image, &CreateState::Prepared).is_err());
    admit(&private, &spec.profile.image, &CreateState::Requested).unwrap();
    let password = private_file(&root.path().join("pull"), "pull-secret");
    let login = |server: &str| crate::cloud_runtime::settings::RegistryPull {
        server: server.into(),
        username: "pull".into(),
        password_file: password.clone(),
    };
    private.hetzner.as_mut().unwrap().registry_pull = Some(login("registry.example"));
    assert!(pull(&private, &spec.profile.image).unwrap());
    private.hetzner.as_mut().unwrap().registry_pull = Some(login("other.example"));
    assert_eq!(
        pull(&private, &spec.profile.image).unwrap_err().to_string(),
        "hetzner.registry_pull names a different registry than the profile's image"
    );
    let mut hub = spec.profile.clone();
    hub.image = "team/worker".into();
    settings.hetzner.as_mut().unwrap().registry_pull = Some(login("index.docker.io"));
    assert!(pull(&settings, &hub.image).unwrap());
    let mut explicit = hub.clone();
    explicit.image = "registry-1.docker.io/team/worker".into();
    // Docker would look this login up under registry-1.docker.io.
    assert!(pull(&settings, &explicit.image).is_err());
    settings.hetzner.as_mut().unwrap().registry_pull = Some(login("registry.hub.docker.com"));
    // An alias Docker stores under its own key would leave the pull without credentials.
    assert!(pull(&settings, &hub.image).is_err());
    settings.hetzner.as_mut().unwrap().registry_pull = Some(crate::cloud_runtime::settings::RegistryPull {
        server: "example.azurecr.io".into(),
        username: "pull".into(),
        password_file: root.path().join("missing"),
    });
    // An unreadable pull credential.
    assert!(pull(&settings, &spec.profile.image).is_err());
    preflight(&spec.operation_id, &spec.profile, &settings).unwrap();
    settings.hetzner = None;
    // No Hetzner settings.
    assert!(preflight(&spec.operation_id, &spec.profile, &settings).is_err());
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

fn allowing(locations: &[&str], server_types: &[&str]) -> Allowed {
    Allowed {
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
    use super::readiness::admitted;
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
        deployment::hetzner::{Allowed, Compute, Journal, provision, retained},
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
    fn held_volume() -> String {
        json!({"volume": serde_json::to_value(volume()).unwrap()}).to_string()
    }
    /// The catalog reads that start every fresh request.
    fn catalog() -> Vec<(u16, String)> {
        vec![(200, server_types(4)), (200, locations()), (200, pricing())]
    }
    fn and(mut responses: Vec<(u16, String)>, more: impl IntoIterator<Item = (u16, String)>) -> Vec<(u16, String)> {
        responses.extend(more);
        responses
    }
    /// A fresh request up to its server request: the key, the volume and the checks between.
    fn until_server() -> Vec<(u16, String)> {
        let volume = [(201, created_volume()), (200, free_volume())];
        and(
            and(until_volume(), volume),
            [(200, listing("servers", &json!([]))), (200, free_volume())],
        )
    }
    /// A fresh request up to its volume request.
    fn until_volume() -> Vec<(u16, String)> {
        let key = [(200, listing("ssh_keys", &json!([]))), (201, key())];
        and(and(catalog(), key), [(200, listing("volumes", &json!([])))])
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
        provision_spec(&spec(), responses)
    }

    fn provision_spec(
        spec: &horizon_cloud::WorkerSpec,
        responses: Vec<(u16, String)>,
    ) -> (Deployment, Journal, bool, usize) {
        provision_adjusted(spec, responses, |_, _, _| {})
    }

    /// As `provision_spec`, after `adjust` changes the settings or the saved state.
    fn provision_adjusted(
        spec: &horizon_cloud::WorkerSpec,
        responses: Vec<(u16, String)>,
        adjust: impl FnOnce(&mut Compute, &mut Deployment, &std::path::Path),
    ) -> (Deployment, Journal, bool, usize) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: Deployment = serde_json::from_value(json!({
            // The deployment keeps the canonical cloud ID; `spec` may name another.
            "version": 1, "cloud_id": super::spec().operation_id, "repository": "/fixture", "revision": "a".repeat(40),
            "profile": spec.profile, "stage": "Provision", "operation": {"state": "prepared"}, "spec": spec,
            "worker": null, "sessions": []
        }))
        .unwrap();
        let (address, requests, task) = provider::serve(responses);
        let mut compute = Compute {
            client: Hetzner::loopback(Credential::new("secret-test-key".into()).unwrap(), address).unwrap(),
            settings: serde_json::from_value(
                json!({"token_file": "/unused", "server_types": ["cx33"], "locations": ["hel1"]}),
            )
            .unwrap(),
            allowed: Allowed {
                locations: vec!["hel1".into()],
                server_types: vec!["cx33".into()],
            },
            registries: None,
        };
        adjust(&mut compute, &mut state, root.path());
        store.save(&state).unwrap();
        let result = provision(&compute, &store, &mut state, spec, &Cancellation::default(), &|_| {});
        task.join().unwrap();
        let served = requests.lock().unwrap().len();
        assert!(result.is_err() || served > 0);
        let saved = store.load().unwrap().unwrap();
        let journal = Journal::load(root.path()).unwrap();
        let kept = retained(root.path()).unwrap();
        (saved, journal, kept, served)
    }

    #[test]
    fn a_volume_size_hetzner_refuses_is_refused_before_any_request() {
        let mut small = spec();
        small.profile.storage.volume_gb = 5;
        let (state, journal, kept, served) = provision_spec(&small, Vec::new());
        assert_eq!(served, 0);
        assert!(journal.key.is_none() && !kept);
        assert_eq!(state.operation, CreateState::Prepared);
    }

    #[test]
    fn only_a_request_that_can_still_create_a_server_loads_the_pull_login() {
        let pull = json!({"server": "registry.example", "username": "pull", "password_file": "/missing/pull"});
        // A fresh request needs the login; a requested server is only reconciled,
        // so the login is never read and provisioning goes on to the provider.
        for (operation, responses, expected) in [
            (CreateState::Prepared, Vec::new(), 0),
            (CreateState::Requested, and(catalog(), [error(503, "unavailable")]), 4),
        ] {
            let (state, _, _, served) = provision_adjusted(&spec(), responses, |compute, state, _| {
                compute.settings.registry_pull = serde_json::from_value(pull.clone()).unwrap();
                state.operation = operation.clone();
            });
            assert_eq!((state.operation, served), (operation, expected));
        }
    }

    #[test]
    fn a_cloud_whose_stop_is_unfinished_is_never_reconnected() {
        let bound = CreateState::Bound { worker_id: "42".into() };
        let (state, _, _, served) = provision_adjusted(&spec(), Vec::new(), |_, state, root| {
            state.operation = bound.clone();
            let mut journal = Journal::load(root).unwrap();
            journal.released = Some("42".into());
            journal.save(root).unwrap();
        });
        assert_eq!((state.operation, served), (bound, 0));
    }

    #[test]
    fn a_redeployed_cloud_requests_a_new_volume_after_its_deleted_one() {
        // An unfinished delete, with its key or server left, is refused before any request.
        for (key, operation) in [
            (Some("ssh-ed25519 AAAA"), CreateState::Prepared),
            (None, CreateState::Requested),
        ] {
            let (_, journal, _, served) = provision_adjusted(&spec(), Vec::new(), |_, state, root| {
                let volume = CreateState::Terminated { worker_id: "8".into() };
                let key = key.map(String::from);
                Journal {
                    location: None,
                    volume,
                    key,
                    released: None,
                }
                .save(root)
                .unwrap();
                state.operation = operation.clone();
            });
            assert_eq!((served, journal.location), (0, None));
        }
        let (_, journal, _, _) = provision_adjusted(
            &spec(),
            and(until_volume(), [error(503, "unavailable")]),
            |_, _, root| {
                let deleted = Journal {
                    location: Some("nbg1".into()),
                    volume: CreateState::Terminated { worker_id: "8".into() },
                    key: None,
                    released: None,
                };
                deleted.save(root).unwrap();
            },
        );
        assert_eq!(
            (journal.volume, journal.location.as_deref()),
            (CreateState::Requested, Some("hel1"))
        );
    }

    #[test]
    fn a_spec_for_another_cloud_is_refused_before_any_request() {
        let mut other = spec();
        other.operation_id = "another-cloud".into();
        let (state, journal, kept, served) = provision_spec(&other, Vec::new());
        assert_eq!(served, 0);
        assert!(journal.key.is_none() && !kept);
        assert_eq!(state.operation, CreateState::Prepared);
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
        let (state, journal, kept, _) = provision_with(and(
            catalog(),
            [(200, listing("ssh_keys", &json!([]))), error(503, "unavailable")],
        ));
        // A key may exist on Hetzner, so the cloud keeps it; no volume was
        // requested, so no location is fixed.
        assert!(journal.key.is_some() && kept && journal.location.is_none());
        assert_eq!(
            (journal.volume, state.operation),
            (CreateState::Prepared, CreateState::Prepared)
        );
    }

    #[test]
    fn an_uncertain_volume_request_stays_fenced_for_reconciliation() {
        let (state, journal, kept, _) = provision_with(and(until_volume(), [error(503, "unavailable")]));
        assert_eq!(journal.volume, CreateState::Requested);
        // The location is recorded before the request it fixes.
        assert!(kept && journal.location.as_deref() == Some("hel1"));
        assert_eq!(state.operation, CreateState::Prepared);
    }

    #[test]
    fn an_uncertain_server_request_stays_fenced_with_its_volume_bound() {
        let (state, journal, kept, _) = provision_with(and(until_server(), [error(503, "unavailable")]));
        assert_eq!(journal.volume, CreateState::Bound { worker_id: "9".into() });
        assert_eq!(state.operation, CreateState::Requested);
        assert!(kept && state.worker.is_none());
    }

    #[test]
    fn a_server_below_the_profile_is_bound_but_never_recorded_as_the_worker() {
        let (state, journal, kept, _) =
            provision_with(and(until_server(), [(201, created_server(2)), (200, held_volume())]));
        assert_eq!(
            state.operation,
            CreateState::Bound { worker_id: "42".into() },
            "bound so it can be deleted"
        );
        // An unverified server is never recorded as the worker.
        assert!(state.worker.is_none());
        assert!(kept && matches!(journal.volume, CreateState::Bound { .. }));
    }

    #[test]
    fn a_server_whose_volume_does_not_name_it_is_never_recorded_as_the_worker() {
        let (state, _, kept, _) = provision_with(and(until_server(), [(201, created_server(4)), (200, free_volume())]));
        assert_eq!(state.operation, CreateState::Bound { worker_id: "42".into() });
        assert!(state.worker.is_none() && kept);
    }

    #[test]
    fn a_verified_server_becomes_the_worker() {
        let (state, _, _, _) = provision_with(and(until_server(), [(201, created_server(4)), (200, held_volume())]));
        assert_eq!(state.worker.unwrap().id, "42");
    }
}

#[cfg(unix)]
fn stored(
    root: &std::path::Path,
    operation: &CreateState,
) -> (
    crate::cloud_runtime::state::Store,
    crate::cloud_runtime::state::Deployment,
) {
    let store = crate::cloud_runtime::state::Store::lock(root).unwrap();
    let state: crate::cloud_runtime::state::Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": spec().operation_id, "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec().profile, "stage": "Stopped", "operation": operation, "spec": spec(),
        "worker": worker(&server("running", Some("192.0.2.10")), &spec(), &volume()).unwrap(),
        "sessions": [], "stop_requested": true
    }))
    .unwrap();
    store.save(&state).unwrap();
    (store, state)
}

#[test]
#[cfg(unix)]
fn resuming_clears_only_a_released_servers_fence() {
    use super::lifecycle::resume;
    let root = tempfile::tempdir().unwrap();
    let bound = CreateState::Bound { worker_id: "42".into() };
    let (store, mut state) = stored(root.path(), &bound);
    assert!(
        resume(&store, &mut state).is_err(),
        "a server that was not released is resumed by starting it"
    );
    let mut journal = Journal::load(root.path()).unwrap();
    journal.released = Some("42".into());
    journal.save(root.path()).unwrap();
    state.stage = crate::cloud_runtime::Stage::Stopping;
    assert!(
        resume(&store, &mut state).is_err(),
        "an unfinished stop has not proven the server gone"
    );
    state.stage = crate::cloud_runtime::Stage::Stopped;
    resume(&store, &mut state).unwrap();
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        saved.operation,
        CreateState::Prepared,
        "the next reconnect requests a new server"
    );
    assert_eq!(saved.stage, crate::cloud_runtime::Stage::Readiness);
    assert!(!saved.stop_requested && saved.worker.is_none());
    assert!(Journal::load(root.path()).unwrap().released.is_none());
}

/// Deletion cleans up the record each provisioning failure point leaves.
#[cfg(unix)]
mod deletion_points;
