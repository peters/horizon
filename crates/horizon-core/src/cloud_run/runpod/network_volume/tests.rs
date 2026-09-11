use super::super::http::RunPodHttp;
use super::*;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use ureq::{
    Body, SendBody,
    http::{Request, Response},
    middleware::MiddlewareNext,
};

const OPERATION: &str = "network volume inspection";
const PRIVATE_SENTINEL: &str = "private-provider-response-never-disclosed";

fn expectation() -> RunPodNetworkVolumeExpectation {
    RunPodNetworkVolumeExpectation {
        volume_id: "volume_exact".into(),
        data_center_id: "EU-TEST-1".into(),
        minimum_size_gb: 10,
    }
}

fn metadata() -> Value {
    // Documented v2 GET shape: id, name, size, dataCenter and immutable type.
    json!({"id": "volume_exact", "name": PRIVATE_SENTINEL, "size": 10,
        "dataCenter": "EU-TEST-1", "type": "HIGH_PERFORMANCE"})
}

fn client(status: u16, body: String) -> (RunPodClient, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let agent = ureq::Agent::config_builder()
        .middleware(move |request: Request<SendBody>, _: MiddlewareNext| {
            count.fetch_add(1, Ordering::SeqCst);
            // Any list, Pod inspection, create, Stop or Delete call fails here.
            assert_eq!(request.method(), "GET");
            assert_eq!(request.uri(), "https://api.runpod.io/v2/network-volumes/volume_exact");
            assert_eq!(request.headers()["Authorization"], "Bearer synthetic-credential");
            Ok(Response::builder()
                .status(status)
                .body(Body::builder().data(body.clone()))
                .expect("response"))
        })
        .build()
        .new_agent();
    (RunPodClient::with_transport(RunPodHttp::mock(agent)), calls)
}

#[test]
fn matching_hps_metadata_is_one_exact_read_not_ownership_or_free_space() {
    for size in [10, 64, 4_096] {
        let mut body = metadata();
        body["size"] = json!(size);
        let (client, calls) = client(200, body.to_string());
        let observed = client
            .inspect_high_performance_volume(&expectation())
            .expect("observation")
            .expect("present");
        assert_eq!(
            observed,
            RunPodNetworkVolume {
                volume_id: "volume_exact".into(),
                data_center_id: "EU-TEST-1".into(),
                size_gb: size,
            }
        );
        assert!(!format!("{observed:?}").contains(PRIVATE_SENTINEL));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn invalid_expectations_do_not_send_any_request() {
    let mut invalid = Vec::new();
    for value in ["", "../foreign", "volume?secret", "line\nbreak"] {
        invalid.push(RunPodNetworkVolumeExpectation {
            volume_id: value.into(),
            ..expectation()
        });
        invalid.push(RunPodNetworkVolumeExpectation {
            data_center_id: value.into(),
            ..expectation()
        });
    }
    invalid.push(RunPodNetworkVolumeExpectation {
        volume_id: "x".repeat(192),
        ..expectation()
    });
    invalid.push(RunPodNetworkVolumeExpectation {
        data_center_id: "x".repeat(192),
        ..expectation()
    });
    for size in [0, 9, 4_097, u32::MAX] {
        invalid.push(RunPodNetworkVolumeExpectation {
            minimum_size_gb: size,
            ..expectation()
        });
    }
    for expected in invalid {
        let (client, calls) = client(200, metadata().to_string());
        assert_eq!(
            client.inspect_high_performance_volume(&expected),
            Err(RunPodError::InvalidTarget)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn different_identity_location_tier_or_size_cannot_be_adopted() {
    for (field, value) in [
        ("id", json!("other_volume")),
        ("dataCenter", json!("OTHER-DC")),
        ("type", json!("STANDARD")),
        ("type", json!("UNKNOWN")),
        ("size", json!(9)),
        ("size", json!(4_097)),
    ] {
        let mut body = metadata();
        body[field] = value;
        let (client, calls) = client(200, body.to_string());
        assert_eq!(
            client.inspect_high_performance_volume(&expectation()),
            Err(RunPodError::ResourceIdentityMismatch)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    let (client, _) = client(200, metadata().to_string());
    assert_eq!(
        client.inspect_high_performance_volume(&RunPodNetworkVolumeExpectation {
            minimum_size_gb: 11,
            ..expectation()
        }),
        Err(RunPodError::ResourceIdentityMismatch)
    );
}

#[test]
fn missing_invalid_and_oversized_responses_remain_redacted() {
    let mut invalid = vec![
        PRIVATE_SENTINEL.into(),
        format!("{{\"padding\":\"{}\"}}", "x".repeat(2 * 1024 * 1024)),
    ];
    for field in ["id", "size", "dataCenter", "type"] {
        let mut body = metadata();
        body.as_object_mut().expect("object").remove(field);
        invalid.push(body.to_string());
        let mut body = metadata();
        body[field] = Value::Null;
        invalid.push(body.to_string());
    }
    for size in [json!(-1), json!(1.5), json!("10"), json!(u64::MAX)] {
        let mut body = metadata();
        body["size"] = size;
        invalid.push(body.to_string());
    }
    for body in invalid {
        let (client, calls) = client(200, body);
        let error = client
            .inspect_high_performance_volume(&expectation())
            .expect_err("invalid response");
        assert_eq!(error, RunPodError::InvalidResponse { operation: OPERATION });
        assert!(!error.to_string().contains(PRIVATE_SENTINEL));
        assert!(!format!("{error:?}").contains(PRIVATE_SENTINEL));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn only_404_is_absence_and_status_errors_do_not_expose_bodies() {
    for status in [200, 204, 302, 401, 403, 404, 429, 500] {
        let (client, calls) = client(status, PRIVATE_SENTINEL.into());
        let expected = match status {
            404 => Ok(None),
            200 => Err(RunPodError::InvalidResponse { operation: OPERATION }),
            _ => Err(RunPodError::UnexpectedStatus {
                operation: OPERATION,
                status,
            }),
        };
        let actual = client.inspect_high_performance_volume(&expectation());
        assert_eq!(actual, expected);
        assert!(!format!("{actual:?}").contains(PRIVATE_SENTINEL));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
