use super::{tests::worker_status, *};
use serde_json::json;

fn persistent_status() -> InteractiveWorkerStatus {
    let mut status = worker_status();
    status.worker.target.lifetime = WorkerLifetime::Persistent;
    status.worker.lifetime = InteractiveWorkerLifetime::Persistent;
    status
}

fn request(status: &InteractiveWorkerStatus) -> InteractiveWorkerRequest {
    InteractiveWorkerRequest {
        workflow_id: status.worker.identity.workflow_id,
        job_id: status.worker.identity.job_id,
        target: status.worker.target.clone(),
        ssh_public_key: status.worker.ssh_public_key.clone(),
    }
}

#[test]
fn persistent_attachment_has_no_default_expiry_boundary() {
    let status = persistent_status();
    let request = request(&status);
    assert!(request.is_valid_for(CloudProvider::RunPod));
    assert!(status.worker.has_valid_shape());
    let observed_at = time::OffsetDateTime::UNIX_EPOCH;
    for elapsed in [0, 31, 366, 3_650] {
        assert!(status.is_ready_for(&request, observed_at + time::Duration::days(elapsed)));
    }
}

#[test]
fn persisted_worker_requires_matching_target_and_observed_lifetime() {
    let mut persistent = persistent_status();
    let persistent_request = request(&persistent);
    persistent.worker.lifetime = worker_status().worker.lifetime;
    assert!(!persistent.worker.has_valid_shape());
    assert!(!persistent.is_ready_for(&persistent_request, time::OffsetDateTime::UNIX_EPOCH));

    let mut temporary = worker_status();
    let temporary_request = request(&temporary);
    temporary.worker.lifetime = InteractiveWorkerLifetime::Persistent;
    assert!(!temporary.worker.has_valid_shape());
    assert!(!temporary.is_ready_for(&temporary_request, time::OffsetDateTime::UNIX_EPOCH));
}

#[test]
fn observed_lifetime_preserves_expiry_and_requires_explicit_persistence() {
    let legacy = r#"{"terminate_after":"2020-01-01T00:00:00Z"}"#;
    let lifetime: InteractiveWorkerLifetime = serde_json::from_str(legacy).expect("legacy expiry");
    assert!(lifetime.has_valid_shape());
    assert!(lifetime.as_time_limited().is_some());
    assert_eq!(serde_json::to_string(&lifetime).expect("serialize expiry"), legacy);
    assert_eq!(
        serde_json::to_value(InteractiveWorkerLifetime::Persistent).expect("serialize persistent"),
        json!({"lifetime": "persistent"})
    );
    for invalid in [
        json!({}),
        json!(null),
        json!("persistent"),
        json!({"lifetime": null}),
        json!({"lifetime": "unknown"}),
        json!({"terminate_after": null}),
        json!({"lifetime": "persistent", "terminate_after": "2020-01-01T00:00:00Z"}),
        json!({"lifetime": "persistent", "terminate_after": null}),
        json!({"lifetime": "persistent", "unexpected": true}),
    ] {
        assert!(
            serde_json::from_value::<InteractiveWorkerLifetime>(invalid.clone()).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn persistent_handle_round_trips_without_manufacturing_a_deadline() {
    let status = persistent_status();
    let encoded = serde_json::to_value(&status).expect("serialize persistent status");
    assert_eq!(encoded["worker"]["lease"], json!({"lifetime": "persistent"}));
    assert_eq!(encoded["worker"]["target"]["lifetime"], "persistent");
    assert!(encoded["worker"]["target"].get("lease_seconds").is_none());
    let restored: InteractiveWorkerStatus = serde_json::from_value(encoded).expect("restore persistent status");
    assert_eq!(restored, status);
    assert!(restored.worker.has_valid_shape());
}
