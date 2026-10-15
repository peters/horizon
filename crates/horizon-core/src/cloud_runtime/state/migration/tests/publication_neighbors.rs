use super::*;
use crate::cloud_runtime::{publishing::Publication, state::PUBLISHING_RECORD_VERSION};

#[test]
fn publication_neighbors_retain_publisher_evidence_and_shared_worker_fences() {
    for same_worker in [false, true] {
        let fixture = Fixture::new();
        let own_store = Store::lock(&fixture.root).unwrap();
        let mut own = own_store.load().unwrap().unwrap();
        own.operation = horizon_cloud::CreateState::Bound {
            worker_id: "worker-one".into(),
        };
        own_store.save(&own).unwrap();
        drop(own_store);
        let other_root = fixture.root.parent().unwrap().join("other-cloud");
        let mut other = own;
        other.cloud_id = "other-cloud".into();
        other.operation = horizon_cloud::CreateState::Bound {
            worker_id: if same_worker { "worker-one" } else { "worker-two" }.into(),
        };
        other.profile.image = "registry.example/worker".into();
        other.profile.build = Some(serde_json::from_value(json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
        other.publication = Some(Box::new(Publication {
            profile: "cpu".into(),
            requested_image: String::new(),
            destination: horizon_cloud::image_publishing::Destination::bound(
                "registry.example/worker".into(),
                "synthetic-publishing-grant".into(),
            )
            .unwrap(),
            account: None,
        }));
        Store::lock(&other_root).unwrap().save(&other).unwrap();
        let before = required(&other_root.join(DEPLOYMENT)).unwrap();
        if same_worker {
            assert!(fixture.migrate().is_err());
            assert_eq!(required(&other_root.join(DEPLOYMENT)).unwrap(), before);
        } else {
            drop(fixture.migrate().unwrap());
            assert_eq!(required(&other_root.join(DEPLOYMENT)).unwrap(), before);
            let mut migrated =
                MigratedStore::migrate(&other_root, "saved-session", "workspace", fixture.controller).unwrap();
            let restored = migrated.load().unwrap().deployment();
            assert_eq!(restored.publication, other.publication);
            assert_eq!(restored.version, PUBLISHING_RECORD_VERSION);
        }
    }
}
