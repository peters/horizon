use super::*;
use horizon_core::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        OverlayChange, OverlayContent, RepositoryOverlayPlan,
        bundle::{RepositoryOverlayBundle, VerifiedOverlayBlob, codec},
    },
};
use serde_json::{Value, json};
use std::io;

fn bundle() -> RepositoryOverlayBundle {
    let blob = VerifiedOverlayBlob::new(b"synthetic\0raw\xff".to_vec()).unwrap();
    let change = OverlayChange::new(
        "selected".into(),
        OverlayContent::File {
            sha256: blob.sha256().clone(),
            bytes: blob.bytes().len() as u64,
            executable: true,
        },
    )
    .unwrap();
    let plan = RepositoryOverlayPlan::new(
        GitSource {
            repository: "synthetic/project".into(),
            commit: GitCommitSha::parse("a".repeat(40)).unwrap(),
            branch: None,
        },
        [change],
        [],
    )
    .unwrap();
    RepositoryOverlayBundle::new(plan, [blob]).unwrap()
}

fn request() -> Value {
    json!({"version":1, "bundle_store":std::env::temp_dir().join("synthetic-private-receiver"),
        "bundle_manifest":bundle().manifest_sha256()})
}

fn header() -> Value {
    let mut value = request();
    value["encoded_bytes"] = json!(codec::encode(&bundle()).unwrap().len());
    value
}

fn frame(header: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(header.len()).unwrap().to_le_bytes().to_vec();
    bytes.extend_from_slice(header);
    bytes.extend_from_slice(payload);
    bytes
}

fn assert_rejected(command: Command, mut input: &[u8]) {
    let mut output = Vec::new();
    let mut diagnostics = Vec::new();
    let code = run_with(command, &mut input, &mut output, &mut diagnostics, |_| {
        panic!("invalid input must not reach storage")
    });
    assert_eq!(code, ExitCode::from(2));
    assert!(diagnostics.is_empty() && output.ends_with(b"\n") && output.len() <= RESPONSE_LIMIT);
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["status"], "rejected");
    assert!(response["bundle_manifest"].is_null());
    assert!(!String::from_utf8(output).unwrap().contains("private"));
}

#[test]
fn exact_framed_bundle_and_status_preserve_only_verified_intent() {
    let expected = bundle();
    let payload = codec::encode(&expected).unwrap();
    let input = frame(&serde_json::to_vec(&header()).unwrap(), &payload);
    let operation = request::read(Command::Receive, &mut input.as_slice()).unwrap();
    let Operation::Receive {
        request: received,
        bundle,
    } = operation
    else {
        panic!("receive operation")
    };
    assert_eq!(*bundle, expected);
    assert_eq!(received.bundle_manifest, *expected.manifest_sha256());
    assert_eq!(
        received.bundle_store,
        std::env::temp_dir().join("synthetic-private-receiver")
    );
    let observed = request::read(
        Command::Observe,
        &mut serde_json::to_vec(&request()).unwrap().as_slice(),
    )
    .unwrap();
    assert!(matches!(observed, Operation::Observe(_)));
    assert_eq!(observed.request().bundle_manifest, received.bundle_manifest);
}

#[test]
fn malformed_headers_never_open_storage_for_either_command() {
    let payload = codec::encode(&bundle()).unwrap();
    for (command, original) in [(Command::Receive, header()), (Command::Observe, request())] {
        let check = |encoded: &[u8]| match command {
            Command::Receive => assert_rejected(command, &frame(encoded, &payload)),
            Command::Observe => assert_rejected(command, encoded),
        };
        for key in original.as_object().unwrap().keys() {
            let mut changed = original.clone();
            changed.as_object_mut().unwrap().remove(key);
            check(&serde_json::to_vec(&changed).unwrap());
        }
        for (key, value) in [
            ("version", json!(true)),
            ("version", json!(2)),
            ("version", json!(-1)),
            ("bundle_store", json!("relative-private-marker")),
            ("bundle_store", json!(std::env::temp_dir().join("../private-marker"))),
            ("bundle_store", json!("\0private-marker")),
            ("bundle_manifest", json!("G".repeat(64))),
            ("bundle_manifest", json!("a".repeat(63))),
            ("extra", json!(true)),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            check(&serde_json::to_vec(&changed).unwrap());
        }
        let encoded = serde_json::to_string(&original).unwrap();
        for invalid in [
            b"\xff".to_vec(),
            b"{}".to_vec(),
            b"[]".to_vec(),
            format!("{encoded}{{}}").into_bytes(),
            encoded.replacen('{', "{\"version\":1,", 1).into_bytes(),
        ] {
            check(&invalid);
        }
    }
    assert_rejected(Command::Observe, &serde_json::to_vec(&header()).unwrap());
}

#[test]
fn framing_truncation_trailing_bytes_and_unverified_payload_never_reach_storage() {
    let payload = codec::encode(&bundle()).unwrap();
    let encoded = serde_json::to_vec(&header()).unwrap();
    let valid = frame(&encoded, &payload);
    for length in [0, 1, 3, 4, 4 + encoded.len() - 1, valid.len() - 1] {
        assert_rejected(Command::Receive, &valid[..length]);
    }
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"private-trailing-bytes");
    let mut measured = io::Cursor::new(trailing);
    assert!(request::read(Command::Receive, &mut measured).is_err());
    assert_eq!(measured.position(), valid.len() as u64 + 1);
    let mut corrupted = valid;
    *corrupted.last_mut().unwrap() ^= 1;
    assert_rejected(Command::Receive, &corrupted);
    for (key, value) in [
        ("bundle_manifest", json!("b".repeat(64))),
        ("encoded_bytes", json!(0)),
        ("encoded_bytes", json!(-1)),
        ("encoded_bytes", json!(payload.len() - 1)),
        ("encoded_bytes", json!(payload.len() + 1)),
        ("encoded_bytes", json!(codec::MAX_ENCODED_BUNDLE_BYTES + 1)),
    ] {
        let mut changed = header();
        changed[key] = value;
        assert_rejected(
            Command::Receive,
            &frame(&serde_json::to_vec(&changed).unwrap(), &payload),
        );
    }
}

#[test]
fn header_limits_bound_reads_and_allow_exact_supported_frames() {
    let payload = codec::encode(&bundle()).unwrap();
    let mut encoded = serde_json::to_vec(&header()).unwrap();
    encoded.resize(request::HEADER_LIMIT, b' ');
    assert!(request::read(Command::Receive, &mut frame(&encoded, &payload).as_slice()).is_ok());
    encoded.push(b' ');
    let mut measured = io::Cursor::new(frame(&encoded, &payload));
    assert!(request::read(Command::Receive, &mut measured).is_err());
    assert_eq!(measured.position(), 4);
    assert_rejected(Command::Receive, &0_u32.to_le_bytes());
    let mut status = serde_json::to_vec(&request()).unwrap();
    status.resize(request::HEADER_LIMIT, b' ');
    assert!(request::read(Command::Observe, &mut status.as_slice()).is_ok());
    status.extend_from_slice(b"  ");
    let mut measured = io::Cursor::new(status);
    assert!(request::read(Command::Observe, &mut measured).is_err());
    assert_eq!(measured.position(), request::HEADER_LIMIT as u64 + 1);
}

#[test]
fn read_failures_are_rejections_and_interrupted_eof_can_retry() {
    struct Failed;
    impl Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("private-read-failure"))
        }
    }
    struct InterruptEof<'a>(&'a [u8], bool);
    impl Read for InterruptEof<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() && !self.1 {
                self.1 = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.0.read(bytes)
        }
    }
    for command in [Command::Receive, Command::Observe] {
        assert!(request::read(command, &mut Failed).is_err());
    }
    let payload = codec::encode(&bundle()).unwrap();
    let input = frame(&serde_json::to_vec(&header()).unwrap(), &payload);
    assert!(request::read(Command::Receive, &mut InterruptEof(&input, false)).is_ok());
}

#[cfg(unix)]
#[test]
fn maximum_escaped_root_fits_but_oversized_or_nul_roots_never_reach_storage() {
    use horizon_core::repository_overlay::materialize::MAX_REQUEST_PATH_BYTES;
    let payload = codec::encode(&bundle()).unwrap();
    let mut value = header();
    value["bundle_store"] = json!(format!("/{}", "\u{1}".repeat(MAX_REQUEST_PATH_BYTES - 1)));
    let encoded = serde_json::to_vec(&value).unwrap();
    assert!(encoded.len() > 24 * 1024 && encoded.len() < request::HEADER_LIMIT);
    assert!(request::read(Command::Receive, &mut frame(&encoded, &payload).as_slice()).is_ok());
    for path in [
        "/".into(),
        "/private\0marker".into(),
        format!("/{}", "x".repeat(MAX_REQUEST_PATH_BYTES)),
    ] {
        value["bundle_store"] = json!(path);
        assert_rejected(Command::Receive, &frame(&serde_json::to_vec(&value).unwrap(), &payload));
    }
}

#[test]
fn a_valid_empty_overlay_is_not_confused_with_an_empty_encoding() {
    let plan = RepositoryOverlayPlan::new(bundle().plan().source().clone(), [], []).unwrap();
    let empty = RepositoryOverlayBundle::new(plan, []).unwrap();
    let payload = codec::encode(&empty).unwrap();
    assert!(!payload.is_empty());
    let mut value = header();
    value["bundle_manifest"] = json!(empty.manifest_sha256());
    value["encoded_bytes"] = json!(payload.len());
    assert!(
        request::read(
            Command::Receive,
            &mut frame(&serde_json::to_vec(&value).unwrap(), &payload).as_slice()
        )
        .is_ok()
    );
}

#[test]
fn response_states_and_lost_output_do_not_repeat_storage_operations() {
    for (status, expected) in [
        (Status::Acknowledged, 0),
        (Status::Observed, 0),
        (Status::Missing, 4),
        (Status::Rejected, 2),
        (Status::Error, 1),
        (Status::WriteUnconfirmed, 1),
    ] {
        assert_eq!(Response::new(status, None, None).exit_code(), expected);
    }
    let payload = codec::encode(&bundle()).unwrap();
    let input = frame(&serde_json::to_vec(&header()).unwrap(), &payload);
    let mut calls = 0;
    let code = run_with(
        Command::Receive,
        &mut input.as_slice(),
        &mut [0_u8; 0].as_mut_slice(),
        &mut io::sink(),
        |operation| {
            calls += 1;
            Response::new(
                Status::Acknowledged,
                Some(operation.request().bundle_manifest.clone()),
                None,
            )
        },
    );
    assert_eq!(code, ExitCode::from(3));
    assert_eq!(calls, 1);
}
