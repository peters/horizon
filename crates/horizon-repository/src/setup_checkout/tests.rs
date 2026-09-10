use super::*;
use std::io;

fn selection() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "version": 1, "destination": "published", "intake": {
            "version": 1, "workspace_local_id": "fixture-workspace",
            "workflow_id": "11111111-1111-4111-8111-111111111111",
            "job_id": "22222222-2222-4222-8222-222222222222", "runtime_generation": 1,
            "worker_resource_id": "fixture-worker", "client_key_sha256": "a".repeat(64),
            "source": {"repository": "fixture/repository", "commit": "a".repeat(40), "branch": null},
            "pack": {"sha256": "b".repeat(64), "encoded_bytes": 32},
            "overlay": {"sha256": "c".repeat(64), "encoded_bytes": 1}
        }
    }))
    .unwrap()
}

#[test]
fn binding_is_inert_and_output_failure_is_distinct() {
    let mut bytes = Vec::new();
    assert_eq!(
        run(
            Command::Binding,
            &mut selection().as_slice(),
            &mut bytes,
            &mut io::sink()
        ),
        ExitCode::SUCCESS
    );
    let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["runtime"], "22222222-2222-4222-8222-222222222222");
    assert_eq!(response["binding_sha256"].as_str().unwrap().len(), 64);
    assert!(response["root"].is_null() && response["reason"].is_null());
    assert_eq!(
        run(
            Command::Binding,
            &mut selection().as_slice(),
            &mut [].as_mut_slice(),
            &mut io::sink()
        ),
        ExitCode::from(3)
    );
}

#[test]
fn malformed_duplicate_and_oversized_inputs_have_no_identity() {
    let valid = selection();
    for bytes in [
        b"private-invalid".to_vec(),
        b"{}".to_vec(),
        vec![b' '; SETUP_SELECTION_LIMIT + 1],
        [b"{\"version\":1,".as_slice(), &valid[1..]].concat(),
    ] {
        let mut output = Vec::new();
        assert_eq!(
            run(Command::Binding, &mut bytes.as_slice(), &mut output, &mut io::sink()),
            ExitCode::from(2)
        );
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["reason"], "invalid");
        assert!(response["root"].is_null() && response["binding_sha256"].is_null() && response["runtime"].is_null());
        assert!(!String::from_utf8(output).unwrap().contains("private-invalid"));
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_inspection_preserves_inert_identity() {
    let mut output = Vec::new();
    assert_eq!(
        run(
            Command::Inspect,
            &mut selection().as_slice(),
            &mut output,
            &mut io::sink()
        ),
        ExitCode::from(1)
    );
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["reason"], "unsupported");
    assert!(response["root"].is_null() && response["binding_sha256"].is_string());
}
