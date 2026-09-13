use super::*;
use horizon_core::cloud_run::ArtifactDigest;
use serde_json::{Value, json};
use std::io::{self, Cursor};

fn request() -> Vec<u8> {
    let parent = if cfg!(windows) {
        "C:/synthetic/retained"
    } else {
        "/synthetic/retained"
    };
    serde_json::to_vec(&json!({
        "version":1,"preparation":{"version":1,"workspace_local_id":"fixture",
            "runtime_id":"11111111-1111-4111-8111-111111111111",
            "source":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":"work"},
            "work_branch":"work"},
        "selected":["source.txt"],"complete_base_closure_consent":true,"retained_volume_attested":true,
        "parent":parent,"attempt_name":"first","max_retained_bytes":268_435_456
    }))
    .unwrap()
}

fn generation(request: &CheckpointRequest) -> CheckpointGeneration {
    let digest = ArtifactDigest::sha256(b"synthetic generation");
    let manifest = serde_json::from_value(json!({
        "version":1,"coverage":"synthetic adapter fixture","preparation":request.preparation,
        "complete_base_closure_consent":true,"retained_volume_attested":true,"selected":request.selected,
        "pack":{"base_commit":request.preparation.source.commit,"sha256":digest,"encoded_bytes":7},
        "overlay_manifest":digest,"overlay_record":digest,"overlay_bytes":5,
        "started_at_millis":1,"verified_at_millis":2
    }))
    .unwrap();
    CheckpointGeneration {
        path: request.parent.join(&request.attempt_name),
        manifest_sha256: digest,
        manifest,
    }
}

fn invoke(
    input: &mut impl Read,
    execute: impl FnOnce(&CheckpointRequest) -> Result<CheckpointGeneration, CheckpointFailure>,
) -> (ExitCode, Value) {
    let (mut output, mut diagnostics) = (Vec::new(), Vec::new());
    let code = run_with(input, &mut output, &mut diagnostics, execute);
    assert!(diagnostics.is_empty());
    assert_eq!(output.last(), Some(&b'\n'));
    (code, serde_json::from_slice(&output).unwrap())
}

struct FailedRead;
impl Read for FailedRead {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("private read failure"))
    }
}

#[test]
fn malformed_and_failed_reads_reject_without_execution() {
    let expected = json!({"version":1,"status":"rejected","generation":null,"retained":null,"reason":"invalid"});
    let mut unknown: Value = serde_json::from_slice(&request()).unwrap();
    unknown["unknown"] = json!(true);
    for bytes in [
        b"private malformed input".to_vec(),
        b"{}".to_vec(),
        Vec::new(),
        [request(), b"{}".to_vec()].concat(),
        serde_json::to_vec(&unknown).unwrap(),
    ] {
        assert_eq!(
            invoke(&mut bytes.as_slice(), |_| panic!("decode failure dispatched")),
            (ExitCode::from(2), expected.clone())
        );
    }
    assert_eq!(
        invoke(&mut FailedRead, |_| panic!("read failure dispatched")),
        (ExitCode::from(2), expected)
    );
}

#[test]
fn exact_request_bound_dispatches_once_and_oversized_reads_stop_at_limit_plus_one() {
    let mut bytes = request();
    bytes.resize(REQUEST_LIMIT, b' ');
    let (code, value) = invoke(&mut bytes.as_slice(), |decoded| {
        assert_eq!(decoded.attempt_name, "first");
        Err(CheckpointFailure {
            reason: CheckpointError::Unsupported,
            retained: None,
        })
    });
    assert_eq!(code, ExitCode::from(2));
    assert_eq!(value["reason"], "unsupported");
    bytes.resize(REQUEST_LIMIT * 2, b' ');
    let mut input = Cursor::new(bytes);
    let (code, value) = invoke(&mut input, |_| panic!("oversized input dispatched"));
    assert_eq!((code, value["reason"].as_str()), (ExitCode::from(2), Some("invalid")));
    assert_eq!(input.position(), REQUEST_LIMIT as u64 + 1);
}

#[test]
fn failure_mappings_preserve_retained_locators_and_never_acknowledge_a_generation() {
    for reason in [
        CheckpointError::Invalid,
        CheckpointError::Unsupported,
        CheckpointError::Identity,
        CheckpointError::Changed,
        CheckpointError::Capacity,
        CheckpointError::Storage,
        CheckpointError::Cancelled,
    ] {
        for retained in [None, Some(PathBuf::from("synthetic-retained/first"))] {
            let rejected =
                retained.is_none() && matches!(reason, CheckpointError::Invalid | CheckpointError::Unsupported);
            let status = if rejected { "rejected" } else { "unconfirmed" };
            let expected = json!({"version":1,"status":status,"generation":null,"retained":retained,"reason":reason});
            let actual = invoke(&mut request().as_slice(), |_| {
                Err(CheckpointFailure { reason, retained })
            });
            assert_eq!(actual, (ExitCode::from(if rejected { 2 } else { 1 }), expected));
        }
    }
}

#[test]
fn verified_response_preserves_all_fields_and_output_loss_is_distinct() {
    let bytes = request();
    let expected = generation(&serde_json::from_slice(&bytes).unwrap());
    let (code, response) = invoke(&mut bytes.as_slice(), |decoded| {
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::from_slice::<Value>(&bytes).unwrap()
        );
        Ok(generation(decoded))
    });
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(
        response,
        json!({"version":1,"status":"verified","generation":expected,"retained":null,"reason":null})
    );
    let mut diagnostics = Vec::new();
    let code = run_with(
        &mut bytes.as_slice(),
        &mut [0; 0].as_mut_slice(),
        &mut diagnostics,
        |decoded| Ok(generation(decoded)),
    );
    assert_eq!(code, ExitCode::from(3));
    assert!(
        String::from_utf8(diagnostics)
            .unwrap()
            .contains("retain data and inspect before any retry")
    );
}

#[cfg(unix)]
#[test]
fn serialization_failure_emits_no_partial_response_or_private_path() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let (mut output, mut diagnostics) = (Vec::new(), Vec::new());
    let code = run_with(&mut request().as_slice(), &mut output, &mut diagnostics, |_| {
        Err(CheckpointFailure {
            reason: CheckpointError::Storage,
            retained: Some(OsString::from_vec(vec![0xff]).into()),
        })
    });
    assert_eq!(code, ExitCode::from(3));
    assert!(output.is_empty());
    assert_eq!(
        diagnostics,
        b"Could not write a complete response; retain data and inspect before any retry.\n"
    );
}
