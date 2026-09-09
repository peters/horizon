use super::*;
use serde_json::json;
use std::io;

fn fixture_path() -> &'static str {
    if cfg!(windows) {
        "C:/private/input"
    } else {
        "/private/input"
    }
}

fn wire(command: Command) -> serde_json::Value {
    let mut value = json!({"version":1,
        "pack":{"base_commit":"1".repeat(40),"sha256":"a".repeat(64),"encoded_bytes":32}});
    match command {
        Command::Receive => {
            value["parent"] = json!(fixture_path());
            value["destination"] = json!("ready");
        }
        Command::Observe => value["path"] = json!(fixture_path()),
    }
    value
}

fn encode(command: Command, value: &serde_json::Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(value).unwrap();
    match command {
        Command::Receive => [u32::try_from(bytes.len()).unwrap().to_le_bytes().as_slice(), &bytes].concat(),
        Command::Observe => bytes,
    }
}

fn rejected(command: Command, mut bytes: &[u8]) {
    let mut output = Vec::new();
    assert_eq!(
        run_with(command, &mut bytes, &mut output, &mut io::sink(), |_, _| panic!(
            "invalid request executed"
        )),
        ExitCode::from(2)
    );
    assert!(output.ends_with(b"\n"));
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["status"], "rejected");
    assert!(response["pack"].is_null() && response["retained"].is_null());
    assert!(!String::from_utf8(output).unwrap().contains("private-marker"));
}

#[test]
fn observation_request_parses_without_executing_a_receive() {
    let body = encode(Command::Observe, &wire(Command::Observe));
    let mut output = Vec::new();
    let code = run_with(
        Command::Observe,
        &mut body.as_slice(),
        &mut output,
        &mut io::sink(),
        |request, input| {
            assert!(request.destination.is_none());
            assert_eq!(request.path.to_str(), Some(fixture_path()));
            assert_eq!(input.read(&mut [0]).unwrap(), 0);
            Response::new(Status::Error, "synthetic observation")
        },
    );
    assert_eq!(code, ExitCode::from(1));
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["status"], "error");
    assert!(response["pack"].is_null() && response["retained"].is_null());
}

#[test]
fn receive_parser_consumes_only_the_exact_header_and_leaves_streaming_payload() {
    let header = encode(Command::Receive, &wire(Command::Receive));
    let payload = b"private-marker-payload";
    let mut input = io::Cursor::new([header.as_slice(), payload].concat());
    let request = request::read(Command::Receive, &mut input).unwrap();
    assert_eq!(input.position(), header.len() as u64);
    assert_eq!(request.destination.as_deref(), Some("ready"));
    assert_eq!(request.pack.encoded_bytes, 32);
    let mut retained = Vec::new();
    input.read_to_end(&mut retained).unwrap();
    assert_eq!(retained, payload);
}

#[test]
fn framing_bounds_and_truncation_fail_before_execution() {
    let header = encode(Command::Receive, &wire(Command::Receive));
    for length in [0, 1, 3, 4, header.len() - 1] {
        rejected(Command::Receive, &header[..length]);
    }
    for length in [0_u32, u32::try_from(request::HEADER_LIMIT + 1).unwrap(), u32::MAX] {
        let mut input = io::Cursor::new([length.to_le_bytes().as_slice(), b"private-marker"].concat());
        assert!(request::read(Command::Receive, &mut input).is_err());
        assert_eq!(input.position(), 4);
    }
    rejected(Command::Observe, &vec![b' '; request::HEADER_LIMIT + 1]);
    rejected(
        Command::Observe,
        &[encode(Command::Observe, &wire(Command::Observe)), b"{}".to_vec()].concat(),
    );
    let mut maximum = encode(Command::Observe, &wire(Command::Observe));
    maximum.resize(request::HEADER_LIMIT, b' ');
    assert!(request::read(Command::Observe, &mut maximum.as_slice()).is_ok());
    maximum.push(b' ');
    rejected(Command::Observe, &maximum);
}

#[test]
fn schemas_identities_and_paths_are_strict_before_execution() {
    for command in [Command::Receive, Command::Observe] {
        let original = wire(command);
        for (field, value) in [
            ("version", json!(0)),
            ("version", json!(2)),
            ("version", json!(1.5)),
            ("unknown", json!(true)),
        ] {
            let mut invalid = original.clone();
            invalid[field] = value;
            rejected(command, &encode(command, &invalid));
        }
        for (field, value) in [
            ("base_commit", json!("0".repeat(40))),
            ("base_commit", json!("g".repeat(40))),
            ("base_commit", json!("1".repeat(39))),
            ("base_commit", json!("private-marker")),
            ("sha256", json!("g".repeat(64))),
            ("sha256", json!("a".repeat(63))),
            ("encoded_bytes", json!(0)),
            ("encoded_bytes", json!(-1)),
            ("unknown", json!(true)),
        ] {
            let mut invalid = original.clone();
            invalid["pack"][field] = value;
            rejected(command, &encode(command, &invalid));
        }
        let path_field = match command {
            Command::Receive => "parent",
            Command::Observe => "path",
        };
        for path in [
            "relative".into(),
            "/".into(),
            "/private/../escape".into(),
            "/private/nul\0private-marker".into(),
            format!(
                "/{}",
                "x".repeat(horizon_core::repository_overlay::materialize::MAX_REQUEST_PATH_BYTES)
            ),
        ] {
            let mut invalid = original.clone();
            invalid[path_field] = json!(path);
            rejected(command, &encode(command, &invalid));
        }
        let text = serde_json::to_string(&original)
            .unwrap()
            .replacen('{', "{\"version\":1,", 1);
        let duplicate = match command {
            Command::Receive => [
                u32::try_from(text.len()).unwrap().to_le_bytes().as_slice(),
                text.as_bytes(),
            ]
            .concat(),
            Command::Observe => text.into_bytes(),
        };
        rejected(command, &duplicate);
    }
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        ".git",
        "nul\0private-marker",
        "bad.",
    ] {
        let mut invalid = wire(Command::Receive);
        invalid["destination"] = json!(name);
        rejected(Command::Receive, &encode(Command::Receive, &invalid));
    }
}

#[test]
fn short_reads_and_interrupted_prefix_reads_preserve_framing() {
    struct Failed;
    impl Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("private-marker"))
        }
    }
    struct Chunks {
        bytes: io::Cursor<Vec<u8>>,
        interrupt: bool,
    }
    impl Read for Chunks {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let length = bytes.len().min(1);
            self.bytes.read(&mut bytes[..length])
        }
    }
    let mut input = Chunks {
        bytes: io::Cursor::new(encode(Command::Receive, &wire(Command::Receive))),
        interrupt: true,
    };
    assert!(request::read(Command::Receive, &mut input).is_ok());
    for command in [Command::Receive, Command::Observe] {
        assert!(request::read(command, &mut Failed).is_err());
    }
}

#[test]
fn shared_identity_types_normalize_hexadecimal_case() {
    for command in [Command::Receive, Command::Observe] {
        let mut value = wire(command);
        value["pack"]["base_commit"] = json!("AB".repeat(20));
        value["pack"]["sha256"] = json!("CD".repeat(32));
        let request = request::read(command, &mut encode(command, &value).as_slice()).unwrap();
        assert_eq!(request.pack.base_commit.as_str(), "ab".repeat(20));
        assert_eq!(request.pack.sha256.as_str(), "cd".repeat(32));
    }
}

#[test]
fn output_loss_never_reexecutes_the_operation() {
    struct Lost(usize);
    impl Write for Lost {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let count = bytes.len().min(self.0);
            self.0 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    for remaining in [0, 1, usize::MAX] {
        let calls = std::cell::Cell::new(0);
        let mut diagnostics = Vec::new();
        let result = run_with(
            Command::Observe,
            &mut encode(Command::Observe, &wire(Command::Observe)).as_slice(),
            &mut Lost(remaining),
            &mut diagnostics,
            |_, _| {
                calls.set(calls.get() + 1);
                Response::new(Status::Error, "synthetic outcome")
            },
        );
        assert_eq!(result, ExitCode::from(3));
        assert_eq!(calls.get(), 1);
        assert_eq!(
            diagnostics,
            b"Could not write a complete response; retain data and inspect before any retry.\n"
        );
    }
}

#[test]
fn status_wire_values_round_trip_without_aliases() {
    for name in [
        "acknowledged",
        "observed",
        "rejected",
        "error",
        "unsupported",
        "receive_unconfirmed",
        "unpublished",
        "published_unsynchronized",
        "rename_unconfirmed",
    ] {
        let status: Status = serde_json::from_value(json!(name)).unwrap();
        assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
    }
    assert!(serde_json::from_value::<Status>(json!("running")).is_err());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_execution_is_explicitly_unsupported_without_reading_the_pack() {
    for command in [Command::Receive, Command::Observe] {
        let header = encode(command, &wire(command));
        let mut bytes = header.clone();
        if matches!(command, Command::Receive) {
            bytes.extend_from_slice(b"unread-pack");
        }
        let mut input = io::Cursor::new(bytes);
        let mut output = Vec::new();
        assert_eq!(
            run(command, &mut input, &mut output, &mut io::sink()),
            ExitCode::from(1)
        );
        assert_eq!(input.position(), header.len() as u64);
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["status"], "unsupported");
        assert!(response["pack"].is_null() && response["retained"].is_null());
    }
}

#[test]
fn receive_parents_reserve_space_for_observable_child_candidates() {
    use horizon_core::repository_overlay::{
        checkout::publication::MAX_SIBLING_NAME_BYTES, materialize::MAX_REQUEST_PATH_BYTES,
    };
    let destination = "x".repeat(MAX_SIBLING_NAME_BYTES);
    let maximum_parent = request::MAX_RECEIVE_PARENT_BYTES;
    for fill in ["x", "\u{1}"] {
        for length in [maximum_parent, maximum_parent + 1, MAX_REQUEST_PATH_BYTES] {
            let parent = format!("{}{}", fixture_path(), fill.repeat(length - fixture_path().len()));
            let mut value = wire(Command::Receive);
            value["parent"] = json!(parent);
            value["destination"] = json!(destination);
            let bytes = encode(Command::Receive, &value);
            if length > maximum_parent {
                rejected(Command::Receive, &bytes);
                continue;
            }
            let received = request::read(Command::Receive, &mut bytes.as_slice()).unwrap();
            for child in [destination.as_str(), "repository-seed-abcdef"] {
                let candidate = received.path.join(child);
                assert!(candidate.to_str().unwrap().len() <= request::MAX_PACK_PATH_BYTES);
                let mut observation = wire(Command::Observe);
                observation["path"] = json!(candidate);
                let bytes = encode(Command::Observe, &observation);
                let reopened = request::read(Command::Observe, &mut bytes.as_slice()).unwrap();
                assert_eq!(reopened.path, candidate);
            }
        }
    }
    let mut observation = wire(Command::Observe);
    observation["path"] = json!(format!(
        "{}{}",
        fixture_path(),
        "x".repeat(request::MAX_PACK_PATH_BYTES + 1 - fixture_path().len())
    ));
    rejected(Command::Observe, &encode(Command::Observe, &observation));
}

#[cfg(target_os = "linux")]
#[test]
fn maximum_escaped_retained_paths_fit_without_claiming_verified_data() {
    use horizon_core::repository_overlay::checkout::publication::MAX_SIBLING_NAME_BYTES;
    let path = std::path::PathBuf::from(format!("/{}", "\u{1}".repeat(request::MAX_RECEIVE_PARENT_BYTES - 1)))
        .join("x".repeat(MAX_SIBLING_NAME_BYTES));
    for (name, source, destination) in [
        ("receive_unconfirmed", true, false),
        ("unpublished", true, false),
        ("published_unsynchronized", false, true),
        ("rename_unconfirmed", true, true),
    ] {
        let status = serde_json::from_value(json!(name)).unwrap();
        let response = Response::retained(
            status,
            source.then(|| path.clone()),
            destination.then(|| path.clone()),
            "retained",
        );
        let mut output = Vec::new();
        assert_eq!(
            super::super::write_response(
                &response,
                response.exit_code(),
                super::super::protocol::RESPONSE_LIMIT,
                &mut output,
                &mut io::sink()
            ),
            ExitCode::from(1)
        );
        assert!(output.len() < super::super::protocol::RESPONSE_LIMIT && output.ends_with(b"\n"));
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["status"], name);
        assert!(value["pack"].is_null());
        assert_eq!(value["retained"]["source"], json!(source.then_some(&path)));
        assert_eq!(value["retained"]["destination"], json!(destination.then_some(&path)));
    }
}
