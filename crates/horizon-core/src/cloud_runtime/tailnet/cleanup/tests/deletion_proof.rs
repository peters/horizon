use super::*;
use crate::cloud_runtime::{
    providers::deletion_confirmed as confirmed,
    state::{Deployment, Store},
};
use horizon_cloud::hetzner::cloud::Journal;
use horizon_cloud::{
    WorkerSpec,
    runpod::volumes::{Spec, State},
};
use serde_json::json;

fn fixture(provider: Kind) -> (tempfile::TempDir, Store, Deployment) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::lock(&temp.path().join("cloud")).unwrap();
    let mut state: Deployment = serde_json::from_value(json!({
        "version":1,"cloud_id":"cloud","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
        "stage":"Deleted","operation":{"state":"terminated","worker_id":"worker"},
        "worker":null,"spec":null,"sessions":[]
    }))
    .unwrap();
    state.profile.provider = provider_id(provider);
    state.spec = Some(
        serde_json::from_value(json!({
            "operation_id":"cloud","image_digest":"sha256:synthetic","profile":state.profile,
            "public_key":"synthetic","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]
        }))
        .unwrap(),
    );
    store.save(&state).unwrap();
    (temp, store, state)
}

fn managed_record(spec: &WorkerSpec) -> serde_json::Value {
    json!({"version":1,"worker":spec,"spec":Spec {
        operation_id:spec.operation_id.clone(),size:u32::from(spec.profile.storage.volume_gb),
        data_center_id:"fixture-center".into(),tier:spec.profile.storage.volume_tier,
    },"state":State::Deleted})
}

#[test]
fn each_default_provider_uses_settled_durable_journals_without_credentials() {
    for provider in [Kind::RunPod, Kind::Hetzner, Kind::DockerHost] {
        let (_temp, store, state) = fixture(provider);
        match provider {
            Kind::RunPod => {
                std::fs::write(store.root().join("workspace-volume.required"), []).unwrap();
                std::fs::write(
                    store.root().join("workspace-volume.json"),
                    serde_json::to_vec(&managed_record(state.spec.as_ref().unwrap())).unwrap(),
                )
                .unwrap();
            }
            Kind::Hetzner => {
                let journal = Journal {
                    location: Some("fixture-location".into()),
                    deleting: true,
                    volume: CreateState::Terminated {
                        worker_id: "1234".into(),
                    },
                    ..Default::default()
                };
                std::fs::write(store.root().join("hetzner.json"), serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            Kind::DockerHost => std::fs::write(store.root().join("docker-host.json"), "{}").unwrap(),
        }
        assert!(confirmed(&store, &state).unwrap());
    }
}

#[test]
fn missing_required_owned_journals_and_foreign_provider_records_do_not_confirm_absence() {
    for provider in [Kind::RunPod, Kind::Hetzner, Kind::DockerHost] {
        let (_temp, store, state) = fixture(provider);
        if provider == Kind::RunPod {
            std::fs::write(store.root().join("workspace-volume.required"), []).unwrap();
        }
        assert!(!confirmed(&store, &state).unwrap_or(false));
    }
    let (_temp, store, state) = fixture(Kind::RunPod);
    assert!(
        confirmed(&store, &state).unwrap(),
        "legacy unmanaged storage has no required marker"
    );
    std::fs::write(store.root().join("docker-host.json"), "{}").unwrap();
    assert!(!confirmed(&store, &state).unwrap());
}

#[test]
fn unsettle_or_masked_fields_cannot_replace_a_confirmed_provider_journal() {
    let (_temp, store, state) = fixture(Kind::RunPod);
    let original = managed_record(state.spec.as_ref().unwrap());
    for change in 0..4 {
        let mut value = original.clone();
        match change {
            0 => value["state"] = json!({"state":"requested"}),
            1 => value["worker"]["operation_id"] = json!("other-cloud"),
            2 => value["unknown_resource"] = json!("unconfirmed"),
            _ => value["worker"]["unknown_resource"] = json!("unconfirmed"),
        }
        std::fs::write(
            store.root().join("workspace-volume.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(!confirmed(&store, &state).unwrap_or(false));
    }
    let (_temp, store, state) = fixture(Kind::DockerHost);
    for text in [
        r#"{"volume":true}"#,
        r#"{"host":"other-host"}"#,
        r#"{"unknown_resource":"pending"}"#,
    ] {
        std::fs::write(store.root().join("docker-host.json"), text).unwrap();
        assert!(!confirmed(&store, &state).unwrap());
    }
    let (_temp, store, state) = fixture(Kind::Hetzner);
    for journal in [
        Journal::default(),
        Journal {
            deleting: true,
            key: Some("key-one".into()),
            ..Default::default()
        },
        Journal {
            deleting: true,
            volume: CreateState::Requested,
            ..Default::default()
        },
    ] {
        std::fs::write(store.root().join("hetzner.json"), serde_json::to_vec(&journal).unwrap()).unwrap();
        assert!(!confirmed(&store, &state).unwrap());
    }
}

#[test]
fn oversized_provider_journals_cannot_confirm_deletion() {
    for (provider, path) in [
        (Kind::RunPod, "workspace-volume.json"),
        (Kind::Hetzner, "hetzner.json"),
        (Kind::DockerHost, "docker-host.json"),
    ] {
        let (_temp, store, state) = fixture(provider);
        std::fs::write(store.root().join(path), vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(!confirmed(&store, &state).unwrap_or(false));
    }
}

// Exact pre-cleanup receipt fields and strict decoding. No outer version 5/6 is
// consumed for this additive metadata; older version-4 receipt readers refuse it.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct BeforeCleanup {
    tailnet_id: TailnetId,
    provider: EnrollmentProvider,
    worker_id: WorkerId,
    allocation_request_id: AllocationRequestId,
    device_id: DeviceId,
}

#[test]
fn older_strict_receipt_reader_accepts_unconfirmed_identity_and_refuses_confirmed_metadata() {
    let mut ledger = super::super::super::receipt::tests::populated();
    let receipt = ledger.receipts()[0].clone();
    let original = serde_json::to_value(&receipt).unwrap();
    let old: BeforeCleanup = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(old).unwrap(), original);
    assert!(original.get("node_deleted").is_none());
    ledger.confirm_removed(&receipt).unwrap();
    let confirmed = serde_json::to_value(&ledger.receipts()[0]).unwrap();
    assert_eq!(confirmed["node_deleted"], true);
    assert!(serde_json::from_value::<BeforeCleanup>(confirmed.clone()).is_err());
    let current: EnrollmentReceipt = serde_json::from_value(confirmed).unwrap();
    assert!(current.node_deleted);
    assert_eq!(current.device_id, receipt.device_id);
    assert_eq!(current.tailnet_id, receipt.tailnet_id);
    assert_eq!(current.worker_id, receipt.worker_id);
}

#[test]
fn future_owner_record_versions_cannot_be_masked_or_saved_away_by_cleanup() {
    for version in [5, 6] {
        let (_directory, store, state) = super::fixture();
        let mut value = serde_json::to_value(state).unwrap();
        value["version"] = serde_json::json!(version);
        value["future_owner_admission"] = serde_json::json!({"must_remain":true});
        let path = store.root().join("deployment.json");
        let before = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &before).unwrap();
        let root = store.root().to_owned();
        drop(store);
        let settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":root.join("absent-provider-key"),"ssh_identity_file":root.join("absent-ssh-key"),
            "docker_config":root.join("absent-docker-config"),"registry_pull_auth_id":null,
            "cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap();
        assert!(
            crate::cloud_runtime::deployment::terminate(
                &root,
                &settings,
                &horizon_cloud::Cancellation::default(),
                &|_| panic!("unsupported owner version reached dispatch")
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn confirmed_device_ids_survive_actual_paired_migration_and_reopen() {
    use crate::cloud_runtime::{allocation::ControllerId, state::migration::MigratedStore};
    let (_temporary, store, mut state) = super::fixture();
    let receipt = state.tailnet_enrollment.receipts()[0].clone();
    state.tailnet_enrollment.confirm_removed(&receipt).unwrap();
    store.save(&state).unwrap();
    let root = store.root().to_owned();
    let expected = state.tailnet_enrollment;
    drop(store);
    let controller = ControllerId::generate();
    {
        let mut migrated = MigratedStore::migrate(&root, "saved-session", "workspace", controller).unwrap();
        let records = migrated.load().unwrap();
        assert_eq!(records.deployment().tailnet_enrollment, expected);
        assert_eq!(records.deployment().version, 4);
        let allocation: serde_json::Value = serde_json::from_slice(&records.allocation_bytes().unwrap()).unwrap();
        assert_eq!(allocation["version"], 2);
        assert_eq!(allocation["tailnet_enrollment"]["receipts"][0]["node_deleted"], true);
        let bytes = String::from_utf8(records.allocation_bytes().unwrap()).unwrap();
        assert!(bytes.contains("\"node_deleted\": true"));
        for duplicate in [
            "\"node_deleted\": false, \"node_deleted\": true",
            "\"node_\\u0064eleted\": false, \"node_deleted\": true",
        ] {
            let duplicated = bytes.replace("\"node_deleted\": true", duplicate);
            assert!(
                crate::cloud_runtime::allocation::legacy::Records::decode(
                    duplicated.as_bytes(),
                    &records.project_bytes().unwrap()
                )
                .is_err()
            );
        }
        migrated.save(&records).unwrap();
    }
    let mut reopened = MigratedStore::migrate(&root, "saved-session", "workspace", controller).unwrap();
    assert_eq!(reopened.load().unwrap().deployment().tailnet_enrollment, expected);
}
