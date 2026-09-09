use super::*;
use crate::repository_overlay::seed::{MAX_PACK_PATH_BYTES, receive::PackReceiveLimits};
use serde_json::{Value, json};

fn path_with_bytes(bytes: usize, character: char) -> String {
    let mut path = String::new();
    while path.len() < bytes {
        path.push('/');
        let count = (bytes - path.len()).min(255);
        path.extend(std::iter::repeat_n(character, count));
    }
    path
}

#[test]
fn request_paths_are_normalized_posix_inputs_not_shell_fragments() {
    let mut expected = Expected::new();
    for path in [
        "", "/", "relative", "é", "//a", "/a/", "/a//b", "/a/../b", "/a/./b", "/a\0b",
    ] {
        expected.path = path.into();
        assert_eq!(
            protocol::request(expected.request()),
            Err(RemotePackInspectionError::InvalidRequest)
        );
    }
    for path in [
        format!("/{}", "a".repeat(256)),
        path_with_bytes(MAX_PACK_PATH_BYTES + 1, 'a'),
    ] {
        expected.path = path;
        assert_eq!(
            protocol::request(expected.request()),
            Err(RemotePackInspectionError::InvalidRequest)
        );
    }
    for path in [
        "/a ;$()\"\\\né".into(),
        path_with_bytes(MAX_PACK_PATH_BYTES, 'a'),
        path_with_bytes(MAX_PACK_PATH_BYTES, '\u{1f}'),
    ] {
        expected.path = path;
        let request = protocol::request(expected.request()).expect("bounded literal path");
        assert_eq!(
            serde_json::from_slice::<Value>(&request).expect("request")["path"],
            expected.path
        );
        protocol::response(&expected.response_bytes(), expected.request()).expect("escaped bounded response");
    }
}

#[test]
fn request_identity_uses_the_worker_pack_limits() {
    let mut expected = Expected::new();
    for bytes in [0, 31, PackReceiveLimits::default().encoded_bytes + 1, u64::MAX] {
        expected.bytes = bytes;
        assert_eq!(
            protocol::request(expected.request()),
            Err(RemotePackInspectionError::InvalidRequest)
        );
    }
    for bytes in [32, PackReceiveLimits::default().encoded_bytes] {
        expected.bytes = bytes;
        protocol::request(expected.request()).expect("supported size");
    }
    expected.base = GitCommitSha::parse("0".repeat(40)).expect("zero commit");
    assert_eq!(
        protocol::request(expected.request()),
        Err(RemotePackInspectionError::InvalidRequest)
    );
}

fn reject(value: &Value, expected: &Expected) {
    assert!(matches!(
        protocol::response(&serde_json::to_vec(value).expect("response"), expected.request()),
        Err(RemotePackInspectionError::InvalidResponse)
    ));
}

#[test]
fn response_binds_version_status_paths_identity_counts_and_required_null_fields() {
    let expected = Expected::new();
    for (pointer, invalid) in [
        ("/version", json!(2)),
        ("/version", json!("1")),
        ("/status", json!("received")),
        ("/retained", json!("private-path")),
        ("/reason", json!("failure")),
        ("/pack/path", json!("/other")),
        ("/pack/objects_directory", json!("/other/decoded/objects")),
        (
            "/pack/objects_directory",
            json!(format!("{}/decoded/../decoded/objects", expected.path)),
        ),
        ("/pack/identity/base_commit", json!("c".repeat(40))),
        ("/pack/identity/sha256", json!("d".repeat(64))),
        ("/pack/identity/encoded_bytes", json!(63)),
        ("/pack/objects", json!(0)),
        ("/pack/objects", json!(-1)),
        ("/pack/objects", json!(u64::MAX)),
    ] {
        let mut value = expected.response();
        *value.pointer_mut(pointer).expect("field") = invalid;
        reject(&value, &expected);
    }
    for (pointer, names) in [
        ("", ["version", "status", "pack", "retained", "reason"].as_slice()),
        ("/pack", ["path", "objects_directory", "identity", "objects"].as_slice()),
        ("/pack/identity", ["base_commit", "sha256", "encoded_bytes"].as_slice()),
    ] {
        for name in names {
            let mut value = expected.response();
            value
                .pointer_mut(pointer)
                .expect("object")
                .as_object_mut()
                .expect("map")
                .remove(*name);
            reject(&value, &expected);
        }
        let mut value = expected.response();
        value
            .pointer_mut(pointer)
            .expect("object")
            .as_object_mut()
            .expect("map")
            .insert("unknown".into(), json!(1));
        reject(&value, &expected);
    }
    let mut value = expected.response();
    value["pack"]["objects"] = json!(crate::repository_overlay::seed::MAX_OBJECTS);
    protocol::response(&serde_json::to_vec(&value).expect("response"), expected.request()).expect("count ceiling");
    value["pack"]["objects"] = json!(crate::repository_overlay::seed::MAX_OBJECTS + 1);
    reject(&value, &expected);
}

#[test]
fn duplicate_trailing_malformed_and_oversize_output_is_not_an_observation() {
    let expected = Expected::new();
    let valid = String::from_utf8(expected.response_bytes()).expect("json");
    for output in [
        "private malformed response".to_string(),
        format!("{valid} {{}}"),
        valid.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
        valid.replacen("\"objects\":3", "\"objects\":3,\"objects\":3", 1),
    ] {
        assert!(matches!(
            protocol::response(output.as_bytes(), expected.request()),
            Err(RemotePackInspectionError::InvalidResponse)
        ));
    }
    let mut boundary = expected.response_bytes();
    boundary.resize(protocol::RESPONSE_LIMIT, b' ');
    protocol::response(&boundary, expected.request()).expect("exact response limit");
    boundary.push(b' ');
    assert!(matches!(
        protocol::response(&boundary, expected.request()),
        Err(RemotePackInspectionError::InvalidResponse)
    ));
}
