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
