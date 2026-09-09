use super::*;
#[cfg(not(target_os = "linux"))]
use IntakeState as State;
use std::io;

pub(super) fn request() -> IntakeRequest {
    serde_json::from_value(serde_json::json!({
        "version":1,"workspace_local_id":"fixture-workspace",
        "workflow_id":"11111111-1111-4111-8111-111111111111",
        "job_id":"22222222-2222-4222-8222-222222222222",
        "runtime_generation":1,"worker_resource_id":"fixture-worker",
        "client_key_sha256":"a".repeat(64),
        "source":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":null},
        "pack":{"sha256":"b".repeat(64),"encoded_bytes":32},
        "overlay":{"sha256":"c".repeat(64),"encoded_bytes":1}
    }))
    .unwrap()
}

pub(super) struct Unread;
impl Read for Unread {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("unexpected payload read")
    }
}

#[test]
fn strict_bounded_identity_preserves_canonical_fields_and_redacts_debug() {
    let mut request = request();
    let bytes = request.encode().unwrap();
    assert_eq!(IntakeRequest::decode(&bytes).unwrap(), request);
    for (field, replacement) in [
        ("version", serde_json::json!(2)),
        ("runtime_generation", serde_json::json!(0)),
        ("workspace_local_id", serde_json::json!("../outside")),
        ("worker_resource_id", serde_json::json!("bad\nvalue")),
        ("worker_resource_id", serde_json::json!("")),
        ("worker_resource_id", serde_json::json!("bad value")),
        ("worker_resource_id", serde_json::json!(" worker")),
        ("worker_resource_id", serde_json::json!("worker ")),
        ("worker_resource_id", serde_json::json!("bad\u{a0}value")),
        ("worker_resource_id", serde_json::json!("-worker")),
        ("worker_resource_id", serde_json::json!("x".repeat(513))),
        ("job_id", serde_json::json!(uuid::Uuid::nil())),
        ("extra", serde_json::json!(true)),
        (
            "pack",
            serde_json::json!({"sha256":"b".repeat(64),"encoded_bytes":PACK_LIMIT+1}),
        ),
    ] {
        let mut value = serde_json::to_value(&request).unwrap();
        value[field] = replacement;
        assert!(IntakeRequest::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        if let Ok(invalid) = serde_json::from_value::<IntakeRequest>(value) {
            let received = receive(&invalid, &mut Unread, || false);
            for response in [received, observe(&invalid, || false)] {
                assert_eq!(response.state, IntakeState::Rejected);
                assert_eq!(response.reason, Some(IntakeError::Invalid));
                assert!(response.roots.is_none() && response.pack.is_none() && response.bundle.is_none());
            }
        }
    }
    for bytes in [
        vec![b' '; REQUEST_LIMIT + 1],
        [bytes.clone(), b" {}".to_vec()].concat(),
        [b"{\"version\":1,".as_slice(), &bytes[1..]].concat(),
    ] {
        assert!(IntakeRequest::decode(&bytes).is_err());
    }
    assert!(!format!("{request:?}").contains("fixture"));
    request.worker_resource_id = "x".repeat(512);
    assert_eq!(IntakeRequest::decode(&request.encode().unwrap()).unwrap(), request);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_never_reads_payload() {
    for response in [
        receive(&request(), &mut Unread, || false),
        observe(&request(), || false),
    ] {
        assert_eq!(response.state, State::Unsupported);
        assert!(response.roots.is_none() && response.pack.is_none());
    }
}
