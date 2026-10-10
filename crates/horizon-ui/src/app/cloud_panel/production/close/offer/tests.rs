use super::super::super::Runtime;
use super::*;
use horizon_core::cloud_runtime::state::Deployment;

fn deployment(provider: &str, stage: &str, operation: serde_json::Value, spec: bool) -> Deployment {
    let mut state = serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a".repeat(40),
        "profile": {"provider": provider, "image": "registry.example/worker", "cpu": 4, "memory_gb": 8},
        "stage": stage, "operation": operation, "sessions": []
    });
    if spec {
        state["spec"] = serde_json::json!({
            "operation_id": "fixture", "image_digest": "sha256:fixture", "profile": state["profile"].clone(),
            "public_key": "ssh-ed25519 fixture", "registry_auth_id": null,
            "gpu_types": [], "cpu_flavors": [], "data_centers": [],
        });
    }
    serde_json::from_value(state).unwrap()
}

fn bound(provider: &str) -> Runtime {
    Runtime {
        state: Some(deployment(
            provider,
            "Ready",
            serde_json::json!({"state": "bound", "worker_id": "worker-7"}),
            true,
        )),
        ..Runtime::default()
    }
}

fn offered(primary: Option<Primary>, remove_anyway: bool, reason: Option<&str>) -> Offer {
    Offer {
        primary,
        remove_anyway,
        reason: reason.map(Into::into),
    }
}

#[test]
fn a_cloud_with_resources_offers_their_deletion_first() {
    assert_eq!(
        offer(&bound("runpod"), true, None),
        offered(Some(Primary::Delete), false, None)
    );
}

#[test]
fn a_failed_deletion_offers_to_try_again_or_remove_anyway() {
    assert_eq!(
        offer(
            &bound("runpod"),
            true,
            Some("Could not delete the cloud resources: timeout")
        ),
        offered(
            Some(Primary::Delete),
            true,
            Some("Could not delete the cloud resources: timeout")
        )
    );
}

#[test]
fn a_cloud_without_resources_is_removed_directly() {
    assert_eq!(
        offer(&Runtime::default(), false, None),
        offered(Some(Primary::Remove), false, None),
        "never deployed"
    );
    let deleted = Runtime {
        state: Some(deployment(
            "runpod",
            "Deleted",
            serde_json::json!({"state": "terminated", "worker_id": "worker-7"}),
            true,
        )),
        ..Runtime::default()
    };
    assert_eq!(
        offer(&deleted, true, None),
        offered(Some(Primary::Remove), false, None),
        "already deleted"
    );
    let unrequested = Runtime {
        state: Some(deployment(
            "runpod",
            "Push",
            serde_json::json!({"state": "prepared"}),
            false,
        )),
        ..Runtime::default()
    };
    assert_eq!(
        offer(&unrequested, true, None),
        offered(Some(Primary::Remove), false, None),
        "no worker was requested"
    );
}

#[test]
fn a_refused_removal_without_a_worker_request_offers_only_remove_anyway() {
    let unrequested = Runtime {
        state: Some(deployment(
            "runpod",
            "Push",
            serde_json::json!({"state": "prepared"}),
            false,
        )),
        ..Runtime::default()
    };
    assert_eq!(
        offer(&unrequested, true, Some("Could not remove the cloud: storage remains")),
        offered(None, true, Some("Could not remove the cloud: storage remains"))
    );
}

#[test]
fn an_unknown_resource_state_offers_only_remove_anyway() {
    let unavailable = Runtime {
        state_unavailable: true,
        ..bound("runpod")
    };
    assert_eq!(
        offer(&unavailable, true, None),
        offered(None, true, Some(STATE_UNKNOWN))
    );
    assert_eq!(
        offer(&Runtime::default(), true, None),
        offered(None, true, Some(STATE_UNKNOWN)),
        "deployed, but no record was read"
    );
    assert_eq!(
        offer(
            &unavailable,
            true,
            Some("Could not delete the cloud resources: no settings")
        ),
        offered(None, true, Some("Could not delete the cloud resources: no settings")),
        "the failure says more than the unknown state"
    );
}

#[test]
fn a_busy_cloud_offers_only_cancel() {
    let (_sender, receiver) = std::sync::mpsc::channel();
    let mut runtime = bound("runpod");
    runtime.receiver = Some(receiver);
    runtime.stage = Some(Stage::Provision);
    for failure in [None, Some("Could not delete the cloud resources")] {
        assert_eq!(offer(&runtime, true, failure), offered(None, false, Some(BUSY)));
    }
    runtime.stage = Some(Stage::Ready);
    assert_eq!(
        offer(&runtime, true, None).primary,
        Some(Primary::Delete),
        "a ready connection does not block closing"
    );
}

#[test]
fn what_may_remain_is_named_where_the_record_knows_it() {
    assert_eq!(
        remains(&bound("runpod"), "runpod"),
        "Worker worker-7 and its workspace storage may still exist at RunPod (runpod.io) and cost money until you delete them there."
    );
    assert_eq!(
        remains(&bound("hetzner"), "hetzner"),
        "Server worker-7, its workspace volume and SSH key may still exist at Hetzner (hetzner.com) and cost money until you delete them there."
    );
    assert_eq!(
        remains(&Runtime::default(), "runpod"),
        "A worker and its workspace storage may still exist at RunPod (runpod.io) and cost money until you delete them there."
    );
    let terminated = Runtime {
        state: Some(deployment(
            "runpod",
            "Stopped",
            serde_json::json!({"state": "terminated", "worker_id": "worker-7"}),
            true,
        )),
        ..Runtime::default()
    };
    assert_eq!(
        remains(&terminated, "runpod"),
        "The workspace storage may still exist at RunPod (runpod.io) and cost money until you delete them there."
    );
    assert!(remains(&Runtime::default(), "elsewhere").contains("at the provider"));
}
