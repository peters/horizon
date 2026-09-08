use super::*;
use horizon_core::repository_overlay::retained_setup::{SetupBoundaryError, SetupExecutionError};
use serde_json::{Value, json};
use std::io;

fn request() -> Value {
    let root = std::env::temp_dir();
    json!({"version":1, "retained_root":root.join("synthetic-private-root"),
        "workspace_local_id":"workspace_1", "objects_directory":root.join("synthetic-objects"),
        "bundle_store":root.join("synthetic-bundles"), "bundle_manifest":"a".repeat(64),
        "destination":"ready"})
}

fn accepts(value: &Value) -> bool {
    request::read(&mut serde_json::to_vec(value).unwrap().as_slice()).is_ok()
}

#[test]
fn immutable_intent_input_is_strict_bounded_and_validated_before_io() {
    let original = request();
    assert!(accepts(&original));
    for key in original.as_object().unwrap().keys() {
        let mut changed = original.clone();
        changed.as_object_mut().unwrap().remove(key);
        assert!(!accepts(&changed));
    }
    for (key, value) in [
        ("version", json!(2)),
        ("version", json!(-1)),
        ("version", json!(1.5)),
        ("workspace_local_id", json!("../private-marker")),
        ("objects_directory", json!("relative")),
        ("bundle_store", json!("relative")),
        ("bundle_manifest", json!("G".repeat(64))),
        ("destination", json!(".git")),
        ("destination", json!("../escape")),
        ("extra", json!(true)),
    ] {
        let mut changed = original.clone();
        changed[key] = value;
        assert!(!accepts(&changed));
    }
    let encoded = serde_json::to_string(&original).unwrap();
    for malformed in [
        String::new(),
        format!("{encoded}{{}}"),
        encoded.replacen('{', "{\"version\":1,", 1),
    ] {
        assert!(request::read(&mut malformed.as_bytes()).is_err());
    }
    assert!(request::read(&mut b"\xff".as_slice()).is_err());
    let mut exact = encoded.into_bytes();
    exact.resize(request::REQUEST_LIMIT, b' ');
    assert!(request::read(&mut exact.as_slice()).is_ok());
    exact.push(b' ');
    assert!(request::read(&mut exact.as_slice()).is_err());
    let mut oversized = io::repeat(b' ');
    assert!(request::read(&mut oversized).is_err());
    let mut measured = io::Cursor::new(vec![b' '; request::REQUEST_LIMIT * 2]);
    assert!(request::read(&mut measured).is_err());
    assert_eq!(measured.position(), request::REQUEST_LIMIT as u64 + 1);
}

#[cfg(unix)]
#[test]
fn three_maximum_escaped_paths_fit_request_bounds() {
    use horizon_core::repository_overlay::materialize::MAX_REQUEST_PATH_BYTES;
    let path = format!("/{}", "\u{1}".repeat(MAX_REQUEST_PATH_BYTES - 1));
    let mut value = request();
    for key in ["retained_root", "objects_directory", "bundle_store"] {
        value[key] = json!(path);
    }
    let encoded = serde_json::to_vec(&value).unwrap();
    assert!(encoded.len() > 64 * 1024 && encoded.len() < request::REQUEST_LIMIT);
    assert!(request::read(&mut encoded.as_slice()).is_ok());
}

#[test]
fn command_states_never_conflate_observation_recording_and_execution() {
    let execution = Err(SetupExecutionError::Boundary(SetupBoundaryError::Cancelled));
    let receipt = SetupCompletion::from_execution(&execution);
    for (outcome, state, recording, code) in [
        (Outcome::Rejected, "rejected", "not_acknowledged", 2),
        (
            Outcome::Error("redacted storage failure".into()),
            "error",
            "not_acknowledged",
            1,
        ),
        (Outcome::Absent, "absent", "not_acknowledged", 0),
        (Outcome::ClaimedUnknown, "claimed_unknown", "not_acknowledged", 4),
        (
            Outcome::Completed {
                receipt: receipt.clone(),
                observed: false,
            },
            "completed",
            "acknowledged",
            2,
        ),
        (
            Outcome::Completed {
                receipt: receipt.clone(),
                observed: true,
            },
            "completed",
            "observed",
            2,
        ),
        (
            Outcome::RecordingUnconfirmed {
                reason: "redacted".into(),
                execution: None,
            },
            "recording_unconfirmed",
            "not_acknowledged",
            1,
        ),
        (
            Outcome::RecordingUnconfirmed {
                reason: "redacted".into(),
                execution: Some(receipt.clone()),
            },
            "recording_unconfirmed",
            "not_acknowledged",
            1,
        ),
    ] {
        let response = response::Response::from_outcome(&outcome);
        assert_eq!(response.exit_code(), code);
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["status"], state);
        assert_eq!(value["recording"], recording);
        assert_eq!(value["version"], 1);
        let has_execution = matches!(
            outcome,
            Outcome::Completed { .. } | Outcome::RecordingUnconfirmed { execution: Some(_), .. }
        );
        assert_eq!(!value["execution"].is_null(), has_execution);
        if !value["execution"].is_null() {
            assert_eq!(value["execution"]["state"], "rejected");
            assert!(value["execution"]["checkout"].is_null());
        }
    }
}

#[test]
fn rejection_output_and_flush_failure_remain_redacted_and_distinct() {
    struct FlushFailure;
    impl Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    for command in [Command::Execute, Command::Observe] {
        let mut output = Vec::new();
        assert_eq!(
            run(command, &mut b"private-marker".as_slice(), &mut output, &mut io::sink()),
            ExitCode::from(2)
        );
        assert!(output.ends_with(b"\n"));
        assert!(!String::from_utf8(output.clone()).unwrap().contains("private-marker"));
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap()["status"], "rejected");
        assert_eq!(
            run(
                command,
                &mut b"{}".as_slice(),
                &mut [0_u8; 0].as_mut_slice(),
                &mut io::sink()
            ),
            ExitCode::from(3)
        );
        assert_eq!(
            run(command, &mut b"{}".as_slice(), &mut FlushFailure, &mut io::sink()),
            ExitCode::from(3)
        );
    }
}
