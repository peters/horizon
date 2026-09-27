#[cfg(unix)]
use super::worker;
use super::{Journal, JournalFile as _};
use horizon_cloud::hetzner::cloud::throwaway_public_key;
#[cfg(unix)]
use horizon_cloud::hetzner::{servers::Server, volumes::Volume};
use horizon_cloud::{CreateState, WorkerSpec};

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

// The server and volume fixtures serve the provisioning tests, which need a real
// store and so run on Unix only.
#[cfg(unix)]
fn server(status: &str, address: Option<&str>) -> Server {
    serde_json::from_value(serde_json::json!({
        "id": 42, "name": format!("horizon-cloud-{}", spec().operation_id), "status": status,
        "public_net": {"ipv4": address.map(|ip| serde_json::json!({"ip": ip}))},
        "server_type": {"name": "cx33", "cores": 4, "memory": 8.0, "disk": 80},
        "location": {"name": "hel1"}, "labels": {"horizon-operation": spec().operation_id}, "volumes": [9]
    }))
    .unwrap()
}

#[cfg(unix)]
fn volume() -> Volume {
    serde_json::from_value(serde_json::json!({
        "id": 9, "name": format!("horizon-cloud-{}", spec().operation_id), "size": 50, "location": {"name": "hel1"},
        "server": 42, "linux_device": "/dev/disk/by-id/scsi-0HC_Volume_9", "status": "available",
        "labels": {"horizon-operation": spec().operation_id}
    }))
    .unwrap()
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
        deleting: true,
        vacating: false,
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

/// The provisioning sequence itself is tested in `horizon_cloud::hetzner::cloud`;
/// these check what Horizon decides before it.
#[cfg(unix)]
mod failure_points {
    use super::spec;
    use crate::cloud_runtime::{
        deployment::hetzner::{Allowed, Compute, provision, retained},
        state::{Deployment, Store},
    };
    use horizon_cloud::{Cancellation, CreateState, Credential, hetzner::Hetzner};
    use serde_json::json;

    /// Provisions a cloud recorded at `operation` whose Hetzner endpoint takes no
    /// connections, with `pull` as its pull login; returns the error, the saved
    /// deployment and whether anything is retained.
    fn provision_at(
        spec: &horizon_cloud::WorkerSpec,
        operation: &CreateState,
        pull: Option<&serde_json::Value>,
    ) -> (String, Deployment, bool) {
        let root = tempfile::tempdir().unwrap();
        provision_in(root.path(), spec, operation, pull, false)
    }

    /// As `provision_at`, in `root` and with the deployment's `source_ready`.
    fn provision_in(
        root: &std::path::Path,
        spec: &horizon_cloud::WorkerSpec,
        operation: &CreateState,
        pull: Option<&serde_json::Value>,
        source_ready: bool,
    ) -> (String, Deployment, bool) {
        let store = Store::lock(root).unwrap();
        let mut state: Deployment = serde_json::from_value(json!({
            // The deployment keeps the canonical cloud ID; `spec` may name another.
            "version": 1, "cloud_id": super::spec().operation_id, "repository": "/fixture", "revision": "a".repeat(40),
            "profile": spec.profile, "stage": "Provision", "operation": operation, "spec": spec,
            "worker": null, "sessions": [], "source_ready": source_ready
        }))
        .unwrap();
        store.save(&state).unwrap();
        // A port nothing listens on: any request fails as a transport error.
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let compute = Compute {
            client: Hetzner::loopback(Credential::new("secret-test-key".into()).unwrap(), closed).unwrap(),
            settings: serde_json::from_value(
                json!({"token_file": "/unused", "server_types": ["cx33"], "locations": ["hel1"], "registry_pull": pull}),
            )
            .unwrap(),
            allowed: Allowed {
                locations: vec!["hel1".into()],
                server_types: vec!["cx33".into()],
            },
            registries: None,
        };
        let error = provision(&compute, &store, &mut state, spec, &Cancellation::default(), &|_| {})
            .unwrap_err()
            .to_string();
        (error, store.load().unwrap().unwrap(), retained(root).unwrap())
    }

    #[test]
    fn a_verified_worker_is_saved_with_the_deployment() {
        let listing = |key: &str, items: serde_json::Value| {
            let mut page = json!({"meta": {"pagination": {"next_page": null}}});
            page[key] = items;
            page.to_string()
        };
        let name = format!("horizon-cloud-{}", spec().operation_id);
        let labels = json!({"horizon-operation": spec().operation_id});
        let mut free = serde_json::to_value(super::volume()).unwrap();
        free["server"] = serde_json::Value::Null;
        let price = json!({"location": "hel1", "price_hourly": {"net": "0.01"}, "price_monthly": {"net": "5"}});
        let kind = json!({"name": "cx33", "cores": 4, "memory": 8.0, "disk": 80, "cpu_type": "shared",
            "architecture": "x86", "prices": [price], "locations": [{"name": "hel1", "available": true}]});
        let server = json!({"id": 42, "name": name, "status": "initializing", "public_net": {"ipv4": null},
            "server_type": {"name": "cx33", "cores": 4, "memory": 8.0, "disk": 80},
            "location": {"name": "hel1"}, "labels": labels, "volumes": [9]});
        let responses = vec![
            (200, listing("server_types", json!([kind]))),
            (200, listing("locations", json!([{"name": "hel1", "network_zone": "eu-central"}]))),
            (200, json!({"pricing": {"currency": "EUR", "volume": {"price_per_gb_month": {"net": "0.05"}}, "primary_ips": []}}).to_string()),
            (200, listing("ssh_keys", json!([]))),
            (201, json!({"ssh_key": {"id": 5, "name": name, "public_key": "@PUBLIC_KEY@", "labels": labels}}).to_string()),
            (200, listing("volumes", json!([]))),
            (201, json!({"volume": free, "action": {"id": 1, "status": "success"}}).to_string()),
            (200, json!({"volume": free}).to_string()),
            (200, listing("servers", json!([]))),
            (200, json!({"volume": free}).to_string()),
            (201, json!({"server": server, "action": {"id": 2, "status": "success"}, "next_actions": []}).to_string()),
            (200, json!({"volume": super::volume()}).to_string()),
        ];
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: Deployment = serde_json::from_value(json!({
            "version": 1, "cloud_id": spec().operation_id, "repository": "/fixture", "revision": "a".repeat(40),
            "profile": spec().profile, "stage": "Provision", "operation": {"state": "prepared"}, "spec": spec(),
            "worker": null, "sessions": []
        }))
        .unwrap();
        store.save(&state).unwrap();
        let (address, _, task) = super::provider::serve(responses);
        let compute = Compute {
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
        provision(&compute, &store, &mut state, &spec(), &Cancellation::default(), &|_| {}).unwrap();
        task.join().unwrap();
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.operation, CreateState::Bound { worker_id: "42".into() });
        assert_eq!(saved.worker.unwrap().id, "42");
    }

    #[test]
    fn a_deleted_cloud_that_still_claims_its_old_source_is_not_provisioned_afresh() {
        let root = tempfile::tempdir().unwrap();
        let deleted = crate::cloud_runtime::deployment::hetzner::Journal {
            volume: CreateState::Terminated { worker_id: "8".into() },
            deleting: true,
            ..Default::default()
        };
        crate::cloud_runtime::deployment::hetzner::JournalFile::save(&deleted, root.path()).unwrap();
        let (error, _, _) = provision_in(root.path(), &spec(), &CreateState::Prepared, None, true);
        assert_eq!(error, "Finish deleting this Hetzner cloud before deploying it again");
    }

    #[test]
    fn a_spec_for_another_cloud_is_refused_before_any_request() {
        let mut other = spec();
        other.operation_id = "another-cloud".into();
        let (error, state, kept) = provision_at(&other, &CreateState::Prepared, None);
        assert_eq!(error, "Deployment and worker identities differ");
        assert!(!kept && state.operation == CreateState::Prepared);
    }

    #[test]
    fn only_a_request_that_can_still_create_a_server_loads_the_pull_login() {
        let pull = json!({"server": "registry.example", "username": "pull", "password_file": "/missing/pull"});
        // A fresh request needs the login; a requested server is only reconciled,
        // so the login is never read and provisioning goes on to the provider.
        let transport = horizon_cloud::CloudError::Transport.to_string();
        let (fresh, _, kept) = provision_at(&spec(), &CreateState::Prepared, Some(&pull));
        assert!(
            !kept && fresh != transport,
            "the unreadable login is refused first: {fresh}"
        );
        let (requested, state, _) = provision_at(&spec(), &CreateState::Requested, Some(&pull));
        assert_eq!((requested, state.operation), (transport, CreateState::Requested));
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
/// Idle stop reads the worker's idle record and stops the cloud as Stop does.
#[cfg(unix)]
mod idle;
