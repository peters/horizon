use super::*;
use crate::cloud_runtime::state::ENROLLMENT_RECORD_VERSION;
use horizon_cloud::CreateState;

fn neighbor(fixture: &Fixture, worker: &str) -> (PathBuf, serde_json::Value) {
    let old = Store::lock(&fixture.root).unwrap();
    let mut own = old.load().unwrap().unwrap();
    own.operation = CreateState::Bound {
        worker_id: "worker-one".into(),
    };
    old.save(&own).unwrap();
    let root = fixture.root.parent().unwrap().join("other-cloud");
    std::fs::create_dir(&root).unwrap();
    let mut other = serde_json::to_value(own).unwrap();
    other["cloud_id"] = json!("other-cloud");
    other["operation"] = json!({"state":"bound", "worker_id":worker});
    other["version"] = json!(ENROLLMENT_RECORD_VERSION);
    other["tailnet_enrollment"] = json!({
        "receipts":[{
            "tailnet_id":"saved-tailnet", "provider":"runpod", "worker_id":worker,
            "allocation_request_id":"other-cloud", "device_id":"n-neighbor"
        }],
        "attempts":[]
    });
    (root, other)
}

#[test]
fn unrelated_enrolled_neighbor_does_not_block_migration_or_lose_its_receipt() {
    let fixture = Fixture::new();
    let (root, value) = neighbor(&fixture, "worker-two");
    let other: Deployment = serde_json::from_value(value).unwrap();
    Store::lock(&root).unwrap().save(&other).unwrap();
    let original = required(&root.join(DEPLOYMENT)).unwrap();
    drop(fixture.migrate().unwrap());
    assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), original);
    let mut migrated = MigratedStore::migrate(&root, "saved-session", "workspace", fixture.controller).unwrap();
    assert_eq!(
        migrated.load().unwrap().deployment().tailnet_enrollment,
        other.tailnet_enrollment
    );
}

#[test]
fn enrolled_neighbor_with_the_same_worker_keeps_both_projects_fenced() {
    let fixture = Fixture::new();
    let (root, value) = neighbor(&fixture, "worker-one");
    Store::lock(&root)
        .unwrap()
        .save(&serde_json::from_value(value).unwrap())
        .unwrap();
    let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
    let other = required(&root.join(DEPLOYMENT)).unwrap();
    assert!(fixture.migrate().is_err());
    assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), original);
    assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), other);
}

#[test]
fn invalid_enrollment_version_shapes_are_not_adopted_as_neighbors() {
    for version in [1, ENROLLMENT_RECORD_VERSION, 999] {
        let fixture = Fixture::new();
        let (root, mut value) = neighbor(&fixture, "worker-two");
        value["version"] = json!(version);
        if version == ENROLLMENT_RECORD_VERSION {
            value["tailnet_enrollment"] = json!({"receipts":[], "attempts":[]});
        }
        std::fs::write(root.join(DEPLOYMENT), serde_json::to_vec(&value).unwrap()).unwrap();
        let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        let other = required(&root.join(DEPLOYMENT)).unwrap();
        assert!(fixture.migrate().is_err());
        assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), original);
        assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), other);
    }
}

#[test]
fn duplicate_neighbor_fields_cannot_replace_enrollment_or_worker_evidence() {
    for duplicate in ["version", "tailnet_enrollment", "worker_id", "device_id"] {
        let fixture = Fixture::new();
        let (root, value) = neighbor(&fixture, "worker-two");
        let encoded = serde_json::to_string(&value).unwrap();
        let malformed = match duplicate {
            "version" => format!("{{\"version\":1,{}", &encoded[1..]),
            "tailnet_enrollment" => format!(
                "{{\"tailnet_enrollment\":{{\"receipts\":[],\"attempts\":[]}},{}",
                &encoded[1..]
            ),
            "worker_id" => encoded.replace(
                "\"worker_id\":\"worker-two\"",
                "\"worker_id\":\"worker-one\",\"worker_id\":\"worker-two\"",
            ),
            "device_id" => encoded.replace(
                "\"device_id\":\"n-neighbor\"",
                "\"device_id\":\"n-previous\",\"device_id\":\"n-neighbor\"",
            ),
            _ => unreachable!(),
        };
        assert_ne!(malformed, encoded);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&malformed).unwrap(), value);
        std::fs::write(root.join(DEPLOYMENT), malformed).unwrap();
        let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        let other = required(&root.join(DEPLOYMENT)).unwrap();
        assert!(fixture.migrate().is_err(), "Duplicate {duplicate} must be refused");
        assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), original);
        assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), other);
    }
}

fn service_neighbor(fixture: &Fixture, worker: &str) -> (PathBuf, serde_json::Value) {
    let (root, mut value) = neighbor(fixture, worker);
    value["version"] = json!(super::super::super::service_admission::RECORD_VERSION);
    value["service_admission"] = json!({
        "owner":{"host_instance":uuid::Uuid::new_v4(),"session_id":uuid::Uuid::new_v4(),"workspace_id":"workspace","cloud_id":"other-cloud"},
        "worker_id":worker,"attempt":{"operation_id":uuid::Uuid::new_v4(),"generation":uuid::Uuid::new_v4()},
        "checked":{"host":"device.example.ts.net","address":"100.64.1.2:8123","binding":{"source":"tailnet","desktop":"desktop-one","peer":"peer-one","name":"device.example.ts.net"}},
        "phase":{"state":"armed"}
    });
    (root, value)
}

#[test]
fn retained_service_neighbors_preserve_enrollment_and_owner_in_both_migration_orders() {
    for service_first in [false, true] {
        let fixture = Fixture::new();
        let (root, value) = service_neighbor(&fixture, "worker-two");
        let owner = &value["service_admission"]["owner"];
        let session = owner["session_id"].as_str().unwrap().to_owned();
        let state: Deployment = serde_json::from_value(value).unwrap();
        Store::lock(&root).unwrap().save(&state).unwrap();
        let before = required(&root.join(DEPLOYMENT)).unwrap();
        let migrate_service = || {
            let mut store = MigratedStore::migrate(&root, &session, "workspace", fixture.controller).unwrap();
            let restored = store.load().unwrap().deployment();
            assert_eq!(restored.service_admission, state.service_admission);
            assert_eq!(restored.tailnet_enrollment, state.tailnet_enrollment);
        };
        if service_first {
            migrate_service();
        }
        drop(fixture.migrate().unwrap());
        if !service_first {
            assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), before);
            migrate_service();
        }
    }
}

#[test]
fn service_neighbor_duplicate_worker_and_malformed_or_duplicate_evidence_preserve_both_records() {
    for invalid in [
        "duplicate-worker",
        "missing-admission",
        "wrong-cloud",
        "duplicate-admission",
    ] {
        let fixture = Fixture::new();
        let (root, mut value) = service_neighbor(
            &fixture,
            if invalid == "duplicate-worker" {
                "worker-one"
            } else {
                "worker-two"
            },
        );
        if invalid == "missing-admission" {
            value.as_object_mut().unwrap().remove("service_admission");
        }
        if invalid == "wrong-cloud" {
            value["service_admission"]["owner"]["cloud_id"] = json!("unrelated-cloud");
        }
        let encoded = serde_json::to_string(&value).unwrap();
        let bytes = if invalid == "duplicate-admission" {
            format!("{{\"service_admission\":null,{}", &encoded[1..]).into_bytes()
        } else {
            encoded.into_bytes()
        };
        std::fs::write(root.join(DEPLOYMENT), &bytes).unwrap();
        let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        assert!(fixture.migrate().is_err(), "Invalid {invalid} must not be adopted");
        assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), original);
        assert_eq!(required(&root.join(DEPLOYMENT)).unwrap(), bytes);
    }
}
