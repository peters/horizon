use super::*;
use serde_json::json;

fn snapshot(lifetime_fields: &str) -> String {
    format!(
        "{{\"workflow_id\":\"11111111-1111-4111-8111-111111111111\",\"job_id\":\"22222222-2222-4222-8222-222222222222\",\"pod_id\":\"pod_123456\",\"name\":\"horizon-11111111-1111-4111-8111-111111111111-22222222-2222-4222-8222-222222222222\",\"image\":\"registry.example/worker@sha256:{}\",{lifetime_fields},\"hourly_cost_micros\":420000}}",
        "d".repeat(64)
    )
}

#[test]
fn persistent_and_expired_legacy_snapshots_round_trip_with_exact_bytes() {
    for fields in [
        r#""lifetime":"persistent""#,
        r#""terminate_after":"2000-01-01T00:00:00Z""#,
    ] {
        let encoded = snapshot(fields);
        let worker: RunPodWorker = serde_json::from_str(&encoded).expect("valid snapshot");
        assert_eq!(worker.validate(), Ok(()));
        assert_eq!(serde_json::to_string(&worker).expect("serialize"), encoded);
    }
}

#[test]
fn incomplete_conflicting_null_unknown_and_duplicate_lifetime_fields_are_rejected() {
    for fields in [
        r#""lifetime":null"#,
        r#""terminate_after":null"#,
        r#""lifetime":"unknown""#,
        r#""lifetime":true"#,
        r#""terminate_after":"not a timestamp""#,
        r#""lifetime":"persistent","terminate_after":"2000-01-01T00:00:00Z""#,
        r#""lifetime":"persistent","terminate_after":null"#,
        r#""lifetime":null,"terminate_after":"2000-01-01T00:00:00Z""#,
        r#""lifetime":"persistent","lifetime":"persistent""#,
        r#""terminate_after":"2000-01-01T00:00:00Z","terminate_after":"2000-01-01T00:00:00Z""#,
        r#""lifetime":"persistent","unexpected":true"#,
    ] {
        assert!(
            serde_json::from_str::<RunPodWorker>(&snapshot(fields)).is_err(),
            "accepted {fields}"
        );
    }
    let mut missing: serde_json::Value = serde_json::from_str(&snapshot(r#""lifetime":"persistent""#)).expect("object");
    missing.as_object_mut().expect("object").remove("lifetime");
    assert!(serde_json::from_value::<RunPodWorker>(missing).is_err());
}

#[test]
fn temporary_creation_keeps_the_exact_legacy_payload() {
    let (workflow, job) = (CloudWorkflowId::new(), CloudJobId::new());
    let request = CreatePodRequest::new(workflow, job, &target(), &profile(), resource_name(workflow, job), None)
        .expect("legacy request");
    let deadline = request.terminate_after.as_deref().expect("explicit deadline");
    let expected = format!(
        "{{\"allowedCudaVersions\":[\"12.8\"],\"cloudType\":\"SECURE\",\"containerDiskInGb\":80,\"containerRegistryAuthId\":\"registry_auth-1\",\"dataCenterId\":\"EUR-NO-1\",\"env\":[{{\"key\":\"HORIZON_WORKFLOW_ID\",\"value\":\"{workflow}\"}},{{\"key\":\"HORIZON_JOB_ID\",\"value\":\"{job}\"}},{{\"key\":\"HORIZON_CLOUD_PROTOCOL_VERSION\",\"value\":\"1\"}},{{\"key\":\"HORIZON_TERMINATE_AFTER\",\"value\":{}}}],\"gpuCount\":1,\"gpuTypeIdList\":[\"NVIDIA RTX A4000\",\"NVIDIA RTX A4500\"],\"imageName\":{},\"minDisk\":200,\"minDownload\":250,\"minUpload\":100,\"name\":{},\"ports\":\"22/tcp\",\"startSsh\":true,\"supportPublicIp\":true,\"terminateAfter\":{},\"volumeInGb\":20,\"volumeMountPath\":\"/workspace\"}}",
        json!(deadline),
        json!(target().image),
        json!(request.name),
        json!(deadline),
    );
    assert_eq!(serde_json::to_string(&request).expect("serialize request"), expected);
}
