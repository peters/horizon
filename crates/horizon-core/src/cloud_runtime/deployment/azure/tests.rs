//! The provisioning sequence itself is tested in `horizon_cloud::azure::provision`
//! against recorded ARM answers; these drive it end to end through this crate's
//! records against a loopback ARM endpoint.
use super::{Compute, load, provision, retained};
use crate::cloud_runtime::{
    mutation::State as Mutation,
    state::{Deployment, Store},
};
use base64::Engine as _;
use horizon_cloud::{Cancellation, CreateState, Credential, WorkerSpec, azure::Azure};
use serde_json::json;

mod lifecycle;
mod rebuild;

const SUB: &str = "00000000-0000-4000-8000-000000000001";
const INSTANCE: &str = "3f2c9a1e-5d4b-4c6a-8e7f-0a1b2c3d4e5f";

fn spec() -> WorkerSpec {
    let profile: horizon_cloud::Profile = serde_json::from_value(json!({
        "provider": "azure", "image": "registry.example/worker", "cpu": 4, "memory_gb": 16,
        "storage": {"container_gb": 20, "volume_gb": 64}
    }))
    .unwrap();
    WorkerSpec {
        exact_placement: false,
        operation_id: "azure-fixture".into(),
        image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
        profile,
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".into(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: vec!["Standard_D4s_v3".into()],
        data_centers: vec!["northeurope".into()],
        startup_metadata: None,
    }
}

fn group() -> String {
    format!("/subscriptions/{SUB}/resourceGroups/horizon-ws-azure-fixture")
}

fn record(root: &std::path::Path, spec: &WorkerSpec, operation: &CreateState) -> (Store, Deployment) {
    let store = Store::lock(root).unwrap();
    let state: Deployment = serde_json::from_value(json!({
        "version": 1, "cloud_id": "azure-fixture", "repository": "/fixture", "revision": "a".repeat(40),
        "profile": spec.profile, "stage": "Provision", "operation": operation, "spec": spec,
        "worker": null, "sessions": []
    }))
    .unwrap();
    store.save(&state).unwrap();
    (store, state)
}

fn compute(address: std::net::SocketAddr) -> Compute {
    Compute::with_client(
        Azure::loopback(Credential::new("synthetic-arm-token".into()).unwrap(), SUB, address).unwrap(),
        serde_json::from_value(json!({
            "token_file": "/unused", "subscription_id": SUB,
            "vm_sizes": ["Standard_D4s_v3"], "regions": ["northeurope"]
        }))
        .unwrap(),
        None,
    )
}

/// A loopback address that takes no connections, so any request fails as a transport error.
fn closed() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn vm() -> String {
    json!({
        "id": format!("{}/providers/Microsoft.Compute/virtualMachines/worker", group()),
        "name": "worker", "location": "northeurope", "tags": {"horizon-operation": "azure-fixture"},
        "properties": {
            "vmId": INSTANCE, "provisioningState": "Succeeded",
            "hardwareProfile": {"vmSize": "Standard_D4s_v3"},
            "storageProfile": {"osDisk": {"diskSizeGB": 30}, "dataDisks": [{"lun": 0, "diskSizeGB": 64,
                "managedDisk": {"id": format!("{}/providers/Microsoft.Compute/disks/worker-workspace", group())}}]},
            "instanceView": {"statuses": [{"code": "PowerState/running"}]}
        }
    })
    .to_string()
}

fn deployment(state: &str) -> String {
    json!({
        "id": format!("{}/providers/Microsoft.Resources/deployments/horizon-worker", group()),
        "name": "horizon-worker",
        "properties": {"provisioningState": state, "outputs": {"publicIp": {"type": "String", "value": "192.0.2.10"}}}
    })
    .to_string()
}

fn created_group() -> String {
    json!({"id": group(), "name": "horizon-ws-azure-fixture", "location": "northeurope",
        "properties": {"provisioningState": "Succeeded"}, "tags": {"horizon-operation": "azure-fixture"}})
    .to_string()
}

/// The method and path of each recorded request.
fn lines(requests: &[String]) -> Vec<String> {
    requests
        .iter()
        .map(|request| {
            let line = request.lines().next().unwrap_or_default();
            let mut parts = line.split(' ');
            let method = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default().split('?').next().unwrap_or_default();
            format!(
                "{method} {}",
                path.trim_start_matches(&format!("/subscriptions/{SUB}/resourceGroups/"))
            )
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn a_scripted_create_records_both_fences_and_saves_the_worker() {
    use crate::cloud_runtime::deployment::hetzner::tests::provider::serve;
    let root = tempfile::tempdir().unwrap();
    let spec = spec();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Prepared);
    let (address, requests, task) = serve(vec![
        (404, json!({"error": {"code": "ResourceGroupNotFound"}}).to_string()),
        (201, created_group()),
        (201, deployment("Succeeded")),
        (200, vm()),
        (200, deployment("Succeeded")),
    ]);
    let seen = std::cell::RefCell::new(Vec::new());
    provision(
        &compute(address),
        &store,
        &mut state,
        &spec,
        &Cancellation::default(),
        &|phase| {
            seen.borrow_mut().push(phase);
            if phase == Mutation::Pending {
                // Each create is announced only once its fence is durable.
                assert!(retained(root.path()).unwrap());
            }
            Ok(Mutation::Pending)
        },
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(
        *seen.borrow(),
        [Mutation::Pending, Mutation::Pending, Mutation::Settled]
    );
    let requests = requests.lock().unwrap().clone();
    assert_eq!(
        lines(&requests),
        [
            "GET horizon-ws-azure-fixture",
            "PUT horizon-ws-azure-fixture",
            "PUT horizon-ws-azure-fixture/providers/Microsoft.Resources/deployments/horizon-worker",
            "GET horizon-ws-azure-fixture/providers/Microsoft.Compute/virtualMachines/worker",
            "GET horizon-ws-azure-fixture/providers/Microsoft.Resources/deployments/horizon-worker",
        ]
    );
    let body: serde_json::Value = serde_json::from_str(requests[2].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let custom = body["properties"]["parameters"]["customData"]["value"]
        .as_str()
        .unwrap();
    let bootstrap = base64::engine::general_purpose::STANDARD.decode(custom).unwrap();
    let bootstrap = String::from_utf8(bootstrap).unwrap();
    assert!(bootstrap.contains("docker.io") && bootstrap.contains("168.63.129.16"));
    assert!(bootstrap.contains("HORIZON_CLOUD_OPERATION=azure-fixture"));
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        saved.operation,
        CreateState::Bound {
            worker_id: INSTANCE.into()
        }
    );
    let worker = saved.worker.unwrap();
    assert_eq!(worker.ssh_endpoint().map(|endpoint| endpoint.port()), Some(22));
    assert_eq!(worker.volume_in_gb, Some(64));
    let journal = load(root.path()).unwrap();
    assert_eq!(journal.region.as_deref(), Some("northeurope"));
    assert_eq!(journal.group, CreateState::Bound { worker_id: group() });
    assert!(retained(root.path()).unwrap());
}

#[cfg(unix)]
#[test]
fn a_rerun_after_a_lost_create_answer_only_reads() {
    use crate::cloud_runtime::deployment::hetzner::tests::provider::serve;
    let root = tempfile::tempdir().unwrap();
    let spec = spec();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Prepared);
    // The create's answer is lost: ARM answers 503 and the outcome is unknown.
    let (address, _, task) = serve(vec![(404, String::new()), (201, created_group()), (503, String::new())]);
    let error = provision(
        &compute(address),
        &store,
        &mut state,
        &spec,
        &Cancellation::default(),
        &|_| Ok(Mutation::Pending),
    )
    .unwrap_err();
    task.join().unwrap();
    assert!(error.to_string().contains("reconcile"), "{error}");
    assert_eq!(store.load().unwrap().unwrap().operation, CreateState::Requested);
    let mut state = store.load().unwrap().unwrap();
    let (address, requests, task) = serve(vec![(200, vm()), (200, deployment("Succeeded"))]);
    let seen = std::cell::RefCell::new(Vec::new());
    provision(
        &compute(address),
        &store,
        &mut state,
        &spec,
        &Cancellation::default(),
        &|phase| {
            seen.borrow_mut().push(phase);
            Ok(Mutation::Pending)
        },
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(
        *seen.borrow(),
        [Mutation::Pending, Mutation::Settled],
        "the unresolved create stays announced"
    );
    assert!(
        lines(&requests.lock().unwrap())
            .iter()
            .all(|line| line.starts_with("GET "))
    );
    assert_eq!(
        store.load().unwrap().unwrap().operation,
        CreateState::Bound {
            worker_id: INSTANCE.into()
        }
    );
}

#[test]
fn a_failed_read_or_a_refused_request_leaves_a_resumable_record() {
    // A read that fails before any create records nothing and retains nothing.
    let root = tempfile::tempdir().unwrap();
    let spec = spec();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Prepared);
    let seen = std::cell::RefCell::new(Vec::new());
    let observe = |phase| {
        seen.borrow_mut().push(phase);
        Ok(Mutation::Settled)
    };
    let error = provision(
        &compute(closed()),
        &store,
        &mut state,
        &spec,
        &Cancellation::default(),
        &observe,
    )
    .unwrap_err();
    assert!(error.to_string().contains("transport"), "{error}");
    assert!(!retained(root.path()).unwrap());
    assert!(!root.path().join("azure.json").exists());
    assert_eq!(store.load().unwrap().unwrap().operation, CreateState::Prepared);
    assert!(seen.borrow().is_empty());
    // A requested worker in a bound group only reads, and its fence is kept.
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Requested);
    super::save(
        &horizon_cloud::azure::provision::Journal {
            region: Some("northeurope".into()),
            group: CreateState::Bound { worker_id: group() },
            ..Default::default()
        },
        root.path(),
    )
    .unwrap();
    assert!(
        provision(
            &compute(closed()),
            &store,
            &mut state,
            &spec,
            &Cancellation::default(),
            &observe
        )
        .is_err()
    );
    assert_eq!(store.load().unwrap().unwrap().operation, CreateState::Requested);
    assert!(retained(root.path()).unwrap());
    assert_eq!(seen.borrow().as_slice(), [Mutation::Pending]);
    // A request that cannot succeed writes no journal.
    let root = tempfile::tempdir().unwrap();
    let mut small = spec.clone();
    small.profile.storage.volume_gb = 3;
    let (store, mut state) = record(root.path(), &small, &CreateState::Prepared);
    let error = provision(
        &compute(closed()),
        &store,
        &mut state,
        &small,
        &Cancellation::default(),
        &observe,
    )
    .unwrap_err();
    assert!(error.to_string().contains("between 4 and 32,767"), "{error}");
    assert!(!root.path().join("azure.json").exists());
    // A spec for another cloud is refused before any request.
    let mut other = spec.clone();
    other.operation_id = "another-cloud".into();
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Prepared);
    let error = provision(
        &compute(closed()),
        &store,
        &mut state,
        &other,
        &Cancellation::default(),
        &observe,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "Deployment and worker identities differ");
}

#[test]
fn a_bound_registry_image_needs_its_grant_verified_first() {
    let root = tempfile::tempdir().unwrap();
    let spec = spec();
    let (store, mut state) = record(root.path(), &spec, &CreateState::Prepared);
    let mut compute = compute(closed());
    compute.registries = Some(
        serde_json::from_value(json!({"root": root.path().join("registry"), "bindings": [{
            "repository": "registry.example/worker", "generation": "synthetic-generation", "read_only_confirmed": true,
            "publish": null, "pull": {"username": "reader", "secret_file": root.path().join("pull"), "expires_at": null}
        }]}))
        .unwrap(),
    );
    let error = provision(&compute, &store, &mut state, &spec, &Cancellation::default(), &|_| {
        Ok(Mutation::Settled)
    })
    .unwrap_err();
    assert!(error.to_string().contains("Verify this cloud's read-only"), "{error}");
    assert!(!retained(root.path()).unwrap());
}

/// The live acceptance: creates, or reconciles, one real Azure worker from the
/// settings file in `HORIZON_AZURE_LIVE_SETTINGS`, with its records in
/// `HORIZON_AZURE_LIVE_STATE`, for cloud `HORIZON_AZURE_LIVE_CLOUD` and the worker
/// image digest `HORIZON_AZURE_LIVE_IMAGE`, then waits for the VM to run and prints
/// the worker. A rerun with the same state reconciles and never creates again.
#[test]
#[ignore = "rents an Azure VM; only the live acceptance runs it"]
fn live_create() {
    let variable = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let settings =
        crate::cloud_runtime::settings::Settings::load(variable("HORIZON_AZURE_LIVE_SETTINGS").as_ref()).unwrap();
    let cloud = variable("HORIZON_AZURE_LIVE_CLOUD");
    let image = variable("HORIZON_AZURE_LIVE_IMAGE");
    let key = settings.ssh_identity_file.with_extension("pub");
    let public_key = std::fs::read_to_string(&key).unwrap().trim().to_owned();
    let mut spec = spec();
    spec.operation_id.clone_from(&cloud);
    spec.profile.image = image.split('@').next().unwrap().to_owned();
    spec.image_digest = image;
    spec.public_key = public_key;
    let root = std::path::PathBuf::from(variable("HORIZON_AZURE_LIVE_STATE"));
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::lock(&root).unwrap();
    let mut state = store.load().unwrap().unwrap_or_else(|| {
        let state: Deployment = serde_json::from_value(json!({
            "version": 1, "cloud_id": cloud, "repository": "/live", "revision": "a".repeat(40),
            "profile": spec.profile, "stage": "Provision", "operation": {"state": "prepared"}, "spec": spec,
            "worker": null, "sessions": []
        }))
        .unwrap();
        store.save(&state).unwrap();
        state
    });
    let compute = Compute::new(&settings).unwrap();
    let cancel = Cancellation::default();
    provision(&compute, &store, &mut state, &spec, &cancel, &|phase| {
        eprintln!(
            "mutation {phase:?}; operation {:?}",
            store.load().ok().flatten().map(|s| s.operation)
        );
        Ok(Mutation::Pending)
    })
    .unwrap();
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        panic!("the worker is not bound");
    };
    let region = load(&root).unwrap().region.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(10);
    loop {
        let worker = horizon_cloud::azure::provision::inspect_worker(
            &compute.client,
            &spec,
            &compute.allowed,
            &region,
            &worker_id,
            &cancel,
        )
        .unwrap();
        if worker.status() == horizon_cloud::WorkerStatus::Running || std::time::Instant::now() > deadline {
            println!("{}", serde_json::to_string_pretty(&worker).unwrap());
            assert_eq!(worker.status(), horizon_cloud::WorkerStatus::Running);
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// The live state of task AZV-L1: its settings and its records.
fn live() -> (crate::cloud_runtime::settings::Settings, std::path::PathBuf) {
    let variable = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
    let settings =
        crate::cloud_runtime::settings::Settings::load(variable("HORIZON_AZURE_LIVE_SETTINGS").as_ref()).unwrap();
    (settings, variable("HORIZON_AZURE_LIVE_STATE").into())
}

/// Prints what the provider reports for the live cloud, as Check provider does.
fn live_report(settings: &crate::cloud_runtime::settings::Settings, root: &std::path::Path) -> bool {
    let reconciled =
        crate::cloud_runtime::lifecycle::reconcile(root, settings, None, &Cancellation::default()).unwrap();
    println!(
        "stage {:?}; stop_requested {}; outcome {:?}; worker {}",
        reconciled.state.stage,
        reconciled.state.stop_requested,
        reconciled.report.outcome,
        serde_json::to_string_pretty(&reconciled.report.worker).unwrap()
    );
    reconciled.confirmed_stopped()
}

/// The live stop: deallocates the VM of task AZV-L1 through the cloud's lifecycle,
/// then checks it as Check provider does. The disks and the address stay.
#[test]
#[ignore = "stops a live Azure VM; only the live acceptance runs it"]
fn live_stop() {
    let (settings, root) = live();
    crate::cloud_runtime::lifecycle::stop(&root, &settings, &Cancellation::default()).unwrap();
    assert!(live_report(&settings, &root), "the stop is confirmed");
}

/// The live resume: starts the same VM again, then reads it until it runs.
#[test]
#[ignore = "starts a live Azure VM; only the live acceptance runs it"]
fn live_resume() {
    let (settings, root) = live();
    crate::cloud_runtime::lifecycle::resume(&root, &settings, &Cancellation::default()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_mins(10);
    loop {
        let reconciled =
            crate::cloud_runtime::lifecycle::reconcile(&root, &settings, None, &Cancellation::default()).unwrap();
        let worker = reconciled.report.worker.clone().unwrap();
        if worker.status() == horizon_cloud::WorkerStatus::Running || std::time::Instant::now() > deadline {
            println!("{}", serde_json::to_string_pretty(&worker).unwrap());
            assert_eq!(worker.status(), horizon_cloud::WorkerStatus::Running);
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

/// The live delete: deletes the cloud as Delete does and prints its journal, which
/// must prove the resource group and every tagged resource absent.
#[test]
#[ignore = "deletes a live Azure resource group; only the live acceptance runs it"]
fn live_delete() {
    let (settings, root) = live();
    crate::cloud_runtime::deployment::terminate(&root, &settings, &Cancellation::default(), &|event| {
        eprintln!("{event:?}");
    })
    .unwrap();
    let journal = load(&root).unwrap();
    println!("{}", serde_json::to_string_pretty(&journal).unwrap());
    let state = Store::lock(&root).unwrap().load().unwrap().unwrap();
    assert_eq!(state.stage, crate::cloud_runtime::Stage::Deleted);
    assert!(journal.proves_absent(&state.cloud_id));
    assert!(!retained(&root).unwrap());
}
