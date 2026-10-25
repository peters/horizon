//! The Azure half of an image rebuild through this crate's records against a
//! loopback ARM endpoint: the VM is deleted and its workspace disk kept, the fence is
//! cleared, and the reconnect after it deploys a new VM on the same disk. The request
//! sequence of the release is tested in `horizon_cloud::azure::release`.
use super::super::{
    rebuild::{held, load as load_release, release_with, released, reopen},
    save,
};
use super::*;
use crate::cloud_runtime::{Stage, deployment::hetzner::tests::provider::serve};
use horizon_cloud::azure::provision::Journal;

const NEW_INSTANCE: &str = "6b1d2e3f-4a5b-4c6d-9e8f-7a6b5c4d3e2f";

fn bound() -> CreateState {
    CreateState::Bound {
        worker_id: INSTANCE.into(),
    }
}

/// A cloud whose rebuild is requested: bound to its VM and group, in the `Replace` stage.
fn rebuilding(root: &std::path::Path, journal: &Journal) -> (Store, Deployment) {
    let (store, mut state) = record(root, &spec(), &bound());
    state.stage = Stage::Replace;
    store.save(&state).unwrap();
    save(journal, root).unwrap();
    (store, state)
}

fn group_journal() -> Journal {
    Journal {
        region: Some("northeurope".into()),
        group: CreateState::Bound { worker_id: group() },
        ..Journal::default()
    }
}

/// The VM `instance` as ARM reports it, its workspace disk detached on delete.
fn vm_of(instance: &str) -> String {
    let mut body: serde_json::Value = serde_json::from_str(&vm()).unwrap();
    body["properties"]["vmId"] = json!(instance);
    body["properties"]["storageProfile"]["dataDisks"][0]["deleteOption"] = json!("Detach");
    body.to_string()
}

fn absent() -> (u16, String) {
    (404, json!({"error": {"code": "ResourceNotFound"}}).to_string())
}

fn free_workspace() -> (u16, String) {
    (
        200,
        json!({
            "id": format!("{}/providers/Microsoft.Compute/disks/worker-workspace", group()),
            "name": "worker-workspace", "tags": {"horizon-operation": "azure-fixture"},
            "properties": {"diskState": "Unattached"}
        })
        .to_string(),
    )
}

/// The VM gone, the OS disk gone and the workspace disk free.
fn proven() -> [(u16, String); 3] {
    [absent(), absent(), free_workspace()]
}

/// The provider mutations each request line names, in order.
fn writes(requests: &[String]) -> Vec<String> {
    lines(requests)
        .into_iter()
        .filter(|line| !line.starts_with("GET "))
        .collect()
}

const VM_PATH: &str = "horizon-ws-azure-fixture/providers/Microsoft.Compute/virtualMachines/worker";

#[cfg(unix)]
#[test]
fn a_rebuild_deletes_only_the_vm_and_keeps_its_disk_and_the_replace_stage() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = rebuilding(root.path(), &group_journal());
    state.worker = Some(
        serde_json::from_value(json!({
            "id": INSTANCE, "name": "azure-fixture", "imageName": spec().image_digest, "desiredStatus": "RUNNING",
            "publicIp": "192.0.2.10", "portMappings": {"22": 22}, "vcpuCount": 4, "memoryInGb": 16,
            "containerDiskInGb": 30, "volumeInGb": 64, "volumeMountPath": "/workspace",
            "dataCenterId": "northeurope", "env": {"HORIZON_CLOUD_OPERATION": "azure-fixture"}
        }))
        .unwrap(),
    );
    let journal = std::fs::read(root.path().join("azure.json")).unwrap();
    let mut script = vec![(200, vm_of(INSTANCE)), (200, String::new())];
    script.extend(proven());
    let (address, requests, task) = serve(script);
    assert!(
        !released(&store, &state).unwrap(),
        "the VM is untouched before the release"
    );
    release_with(&compute(address).client, &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert_eq!(writes(&requests.lock().unwrap()), [format!("DELETE {VM_PATH}")]);
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        (saved.stage, saved.stop_requested, &saved.operation),
        (Stage::Replace, false, &bound()),
        "the rebuild's stage is kept and no stop is recorded"
    );
    let worker = saved.worker.as_ref().unwrap();
    assert!(worker.public_ip.is_none() && !worker.is_starting_or_running());
    assert!(released(&store, &saved).unwrap());
    assert_eq!(
        std::fs::read(root.path().join("azure.json")).unwrap(),
        journal,
        "the group record is not changed: the disk, address and network stay"
    );
    assert!(retained(root.path()).unwrap());
}

#[cfg(unix)]
#[test]
fn a_retry_finishes_the_release_without_a_second_delete() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = rebuilding(root.path(), &group_journal());
    // An interrupted attempt sent the delete; ARM has finished it since.
    let (address, requests, task) = serve(proven().to_vec());
    release_with(&compute(address).client, &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert!(writes(&requests.lock().unwrap()).is_empty());
    assert!(
        released(&store, &state).unwrap(),
        "a VM proven gone is released, so the fence can be cleared"
    );
}

#[cfg(unix)]
#[test]
fn a_vm_the_release_refuses_is_never_recorded_so_a_cancel_keeps_it() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = rebuilding(root.path(), &group_journal());
    // The fixture's data disk names no delete option, so its delete could take the disk.
    let (address, requests, task) = serve(vec![(200, vm())]);
    let refusal = release_with(&compute(address).client, &store, &mut state, &Cancellation::default())
        .unwrap_err()
        .to_string();
    task.join().unwrap();
    assert!(refusal.contains("would delete its workspace disk"), "{refusal}");
    assert!(writes(&requests.lock().unwrap()).is_empty());
    assert!(!released(&store, &state).unwrap());
    assert!(!root.path().join("azure-release.json").exists());
    assert!(!held(root.path()).unwrap());
}

#[cfg(unix)]
#[test]
fn a_cloud_being_deleted_is_not_released() {
    let root = tempfile::tempdir().unwrap();
    let deleting = Journal {
        deleting: true,
        ..group_journal()
    };
    let (store, mut state) = rebuilding(root.path(), &deleting);
    let refusal = release_with(&compute(closed()).client, &store, &mut state, &Cancellation::default())
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("being deleted"), "{refusal}");
    assert!(!root.path().join("azure-release.json").exists());
}

#[cfg(unix)]
#[test]
fn only_the_released_vm_is_reopened_and_the_disk_stays_held() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = rebuilding(root.path(), &group_journal());
    let refusal = reopen(&store, &mut state).unwrap_err().to_string();
    assert!(refusal.contains("not been released"), "{refusal}");
    assert_eq!(store.load().unwrap().unwrap().operation, bound());

    let (address, _, task) = serve(proven().to_vec());
    release_with(&compute(address).client, &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    (state.stage, state.stop_requested) = (Stage::Readiness, true);
    reopen(&store, &mut state).unwrap();
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.operation, CreateState::Prepared);
    assert!(saved.worker.is_none() && !saved.stop_requested);
    let release = load_release(root.path()).unwrap();
    assert!(release.released.is_none(), "the release is cleared with the fence");
    assert!(
        release.held,
        "a VM held this disk, so a reconnect may make the next one"
    );
    assert!(!released(&store, &saved).unwrap());
    assert!(retained(root.path()).unwrap(), "the group, disk and address stay");
}

#[cfg(unix)]
#[test]
fn the_deploy_after_a_rebuild_makes_a_new_vm_on_the_same_disk_and_address() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = rebuilding(root.path(), &group_journal());
    let (address, _, task) = serve(proven().to_vec());
    release_with(&compute(address).client, &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    reopen(&store, &mut state).unwrap();

    let spec = spec();
    let (address, requests, task) = serve(vec![
        // The group is bound, so no group request; the VM is gone.
        absent(),
        (200, deployment("Succeeded")),
        (201, deployment("Succeeded")),
        (200, vm_of(NEW_INSTANCE)),
        (200, deployment("Succeeded")),
    ]);
    provision(
        &compute(address),
        &store,
        &mut state,
        &spec,
        &Cancellation::default(),
        &|_| Ok(Mutation::Settled),
    )
    .unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(
        writes(&requests),
        ["PUT horizon-ws-azure-fixture/providers/Microsoft.Resources/deployments/horizon-worker"],
        "one deployment and nothing deleted"
    );
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        saved.operation,
        CreateState::Bound {
            worker_id: NEW_INSTANCE.into()
        }
    );
    let worker = saved.worker.unwrap();
    assert_eq!(
        (worker.id.as_str(), worker.public_ip.map(|ip| ip.to_string())),
        (NEW_INSTANCE, Some("192.0.2.10".to_owned())),
        "a new VM with the static address it kept"
    );
    let put = requests.iter().find(|request| request.starts_with("PUT ")).unwrap();
    let body: serde_json::Value = serde_json::from_str(put.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let resources = body["properties"]["template"]["resources"].as_array().unwrap();
    let vm = resources
        .iter()
        .find(|resource| resource["type"] == "Microsoft.Compute/virtualMachines")
        .unwrap();
    assert_eq!(
        vm["properties"]["storageProfile"]["dataDisks"],
        json!([{"lun": 0, "createOption": "Attach", "deleteOption": "Detach",
            "managedDisk": {"id": "[resourceId('Microsoft.Compute/disks', 'worker-workspace')]"}}]),
        "the new VM attaches the kept workspace disk on LUN 0"
    );
}

#[cfg(unix)]
#[test]
fn the_next_vm_pulls_with_the_azure_login_and_never_needs_hetzner_settings() {
    use super::super::rebuild::pull_login;
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let private = |name: &str, text: &str| {
        let path = root.path().join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    };
    let token = private("arm-token", "synthetic-arm-token");
    let password = private("pull-password", "synthetic-pull-token");
    let settings = |pull: serde_json::Value| -> crate::cloud_runtime::settings::Settings {
        serde_json::from_value(json!({
            "runpod_key_file": "/unused", "ssh_identity_file": "/unused", "docker_config": "/unused",
            "cpu_flavors": [], "gpu_types": [],
            "azure": {"token_file": token, "subscription_id": SUB, "vm_sizes": ["Standard_D4s_v3"],
                "regions": ["northeurope"], "registry_pull": pull}
        }))
        .unwrap()
    };
    let image = spec().image_digest;
    // A public image needs no login, and no hetzner section is read.
    assert!(
        pull_login(&settings(serde_json::Value::Null), &image)
            .unwrap()
            .is_none()
    );
    let pull = json!({"server": "registry.example", "username": "reader", "password_file": password});
    let login = pull_login(&settings(pull), &image).unwrap().unwrap();
    assert_eq!(login.server, "registry.example");
    let other = json!({"server": "other.example", "username": "reader", "password_file": password});
    let refusal = pull_login(&settings(other), &image).unwrap_err().to_string();
    assert!(refusal.contains("different registry"), "{refusal}");
}
