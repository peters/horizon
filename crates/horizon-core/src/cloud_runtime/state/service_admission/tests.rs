use super::*;
use crate::cloud_runtime::{
    allocation::{AllocationId, ControllerId, ProjectId, ProjectIdentity, legacy::Records},
    state::{Deployment, ENROLLMENT_RECORD_VERSION, Store},
    tailnet::receipt::tests::populated,
};
use serde_json::{Value, json};

fn retained() -> Record {
    serde_json::from_value(json!({
        "owner":{"host_instance":Uuid::new_v4(),"session_id":Uuid::new_v4(),"workspace_id":"workspace-a","cloud_id":"service-cloud"},
        "worker_id":"worker-a","attempt":{"operation_id":Uuid::new_v4(),"generation":Uuid::new_v4()},
        "checked":{"host":"device.example.ts.net","address":"100.64.1.2:8123","binding":{"source":"tailnet","desktop":"desktop-a","peer":"peer-a","name":"device.example.ts.net"}},
        "phase":{"state":"armed"}
    })).unwrap()
}
fn deployment() -> Deployment {
    serde_json::from_value(json!({
        "version":1,"cloud_id":"service-cloud","repository":"/synthetic","revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker-a"},"spec":null,
        "worker":null,"sessions":[],"source_ready":true
    }))
    .unwrap()
}
fn split(state: &Deployment, owner: &OwnerScope) -> Records {
    Records::from_legacy(
        &serde_json::to_vec(state).unwrap(),
        ProjectIdentity::new(
            ProjectId::generate(),
            owner.session_id.to_string(),
            owner.workspace_id.clone(),
            owner.cloud_id.clone(),
        )
        .unwrap(),
        AllocationId::generate(),
        ControllerId::generate(),
    )
    .unwrap()
}
#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn one_service_nonempty_record_fences_old_readers_and_defaults_keep_legacy_versions() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::lock(temp.path()).unwrap();
    let mut state = deployment();
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap().unwrap().version, 1);
    state.service_admission = Some(retained());
    store.save(&state).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.version, RECORD_VERSION);
    assert_eq!(loaded.service_admission, state.service_admission);
    let mut old_encoding = serde_json::to_value(&loaded).unwrap();
    old_encoding["version"] = 1.into();
    std::fs::write(
        temp.path().join("deployment.json"),
        serde_json::to_vec(&old_encoding).unwrap(),
    )
    .unwrap();
    assert!(store.load().is_err(), "old version must not carry erasable admission");
    old_encoding["version"] = RECORD_VERSION.into();
    old_encoding.as_object_mut().unwrap().remove("service_admission");
    std::fs::write(
        temp.path().join("deployment.json"),
        serde_json::to_vec(&old_encoding).unwrap(),
    )
    .unwrap();
    assert!(store.load().is_err(), "new version requires nonempty evidence");
}
#[test]
fn one_service_combined_enrollment_and_admission_are_lossless_after_worker_removal() {
    let record = retained();
    let mut state = deployment();
    state.version = RECORD_VERSION;
    state.service_admission = Some(record.clone());
    state.tailnet_enrollment = populated();
    state.working_copy = Some(crate::cloud_runtime::repository::uncommitted::Confirmed {
        hash: Some("c".repeat(64)),
    });
    let pair = split(&state, record.owner());
    let allocation: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
    assert_eq!(allocation["version"], 4);
    let restored = Records::decode(&pair.allocation_bytes().unwrap(), &pair.project_bytes().unwrap()).unwrap();
    let roundtrip = restored.deployment();
    assert_eq!(roundtrip.service_admission, Some(record.clone()));
    assert_eq!(roundtrip.tailnet_enrollment, state.tailnet_enrollment);
    assert_eq!(roundtrip.working_copy, state.working_copy);
    state.worker = None;
    state.operation = horizon_cloud::CreateState::Prepared;
    let removed = split(&state, record.owner()).deployment();
    assert_eq!(removed.service_admission, Some(record));
    assert_eq!(removed.tailnet_enrollment, state.tailnet_enrollment);
    assert_eq!(removed.working_copy, state.working_copy);
}
#[test]
fn one_service_paired_records_reject_owner_moves_and_version_downgrades() {
    let record = retained();
    let mut state = deployment();
    state.version = RECORD_VERSION;
    state.service_admission = Some(record.clone());
    let pair = split(&state, record.owner());
    let mut allocation: Value = serde_json::from_slice(&pair.allocation_bytes().unwrap()).unwrap();
    allocation["version"] = 2.into();
    assert!(
        Records::decode(
            &serde_json::to_vec(&allocation).unwrap(),
            &pair.project_bytes().unwrap()
        )
        .is_err()
    );
    allocation["version"] = 4.into();
    allocation["service_admission"]["owner"]["workspace_id"] = "other-workspace".into();
    assert!(
        Records::decode(
            &serde_json::to_vec(&allocation).unwrap(),
            &pair.project_bytes().unwrap()
        )
        .is_err()
    );
    let mut enrollment = deployment();
    enrollment.tailnet_enrollment = populated();
    enrollment.version = ENROLLMENT_RECORD_VERSION;
    assert_eq!(
        split(&enrollment, record.owner()).deployment().version,
        ENROLLMENT_RECORD_VERSION
    );
}
#[test]
fn one_service_malformed_retained_id_binding_and_unknown_fields_are_refused() {
    let original = serde_json::to_value(retained()).unwrap();
    for pointer in ["/owner/session_id", "/attempt/operation_id", "/attempt/generation"] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = Uuid::nil().to_string().into();
        assert!(serde_json::from_value::<Record>(value).is_err());
    }
    for pointer in [
        "/worker_id",
        "/checked/host",
        "/checked/binding/desktop",
        "/checked/binding/peer",
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = "x".repeat(1024).into();
        assert!(serde_json::from_value::<Record>(value).is_err());
    }
    let mut value = original.clone();
    value["owner"]["approved"] = true.into();
    assert!(serde_json::from_value::<Record>(value).is_err());
    let mut value = original;
    value["checked"]["address"] = "127.0.0.1:8123".into();
    assert!(serde_json::from_value::<Record>(value).is_err());
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn publication_and_unresolved_admission_keep_both_fences_and_the_current_snapshot_contract() {
    use crate::cloud_runtime::publishing::Publication;
    use horizon_cloud::image_publishing::Destination;

    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert!(Store::snapshot(&missing).unwrap().is_none());
    assert!(!missing.exists());
    let store = Store::lock(root.path()).unwrap();
    let record = retained();
    let mut state = deployment();
    state.service_admission = Some(record.clone());
    state.profile.build = Some(serde_json::from_value(json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
    state.publication = Some(Box::new(Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination: Destination::bound(state.profile.image.clone(), "synthetic-grant".into()).unwrap(),
        account: None,
    }));
    store.save(&state).unwrap();
    let path = root.path().join("deployment.json");
    let before = std::fs::read(&path).unwrap();
    let loaded = Store::snapshot(root.path()).unwrap().unwrap();
    assert_eq!(loaded.version, RECORD_VERSION);
    assert_eq!(loaded.service_admission, state.service_admission);
    assert_eq!(loaded.publication, state.publication);
    assert!(Store::lock_existing(root.path()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let restored = split(&loaded, record.owner()).deployment();
    assert_eq!(restored.version, RECORD_VERSION);
    assert_eq!(restored.service_admission, state.service_admission);
    assert_eq!(restored.publication, state.publication);
    let mut invalid = loaded;
    invalid.profile.image = "registry.example/other".into();
    assert!(store.save(&invalid).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(Store::snapshot(root.path()).is_err());
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn all_retained_phases_survive_storage_without_creating_runtime_authority() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    for phase in [
        json!({"state":"armed"}),
        json!({"state":"attached","proxy_port":1234}),
        json!({"state":"closed"}),
    ] {
        let mut encoded = serde_json::to_value(retained()).unwrap();
        encoded["phase"] = phase;
        let record: Record = serde_json::from_value(encoded).unwrap();
        let mut state = deployment();
        state.service_admission = Some(record.clone());
        store.save(&state).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.version, RECORD_VERSION);
        assert_eq!(loaded.service_admission, Some(record.clone()));
        assert_eq!(
            split(&loaded, record.owner()).deployment().service_admission,
            Some(record)
        );
    }
}
