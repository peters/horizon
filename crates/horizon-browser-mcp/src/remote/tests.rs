//! The bridge reads the record of a host it did not start, so these tests use a
//! recorded answer of a real `horizon-browser host` and the JSON of a real tool
//! result rather than a live machine.

use serde_json::{Value, json};

use super::{RemoteBridgeError, parse_probe};

const RECORD: &str =
    r#"{"host_instance":"7c1f8a9e-2b44-4f4c-9d3f-3c0b6f6f2a11","pid":4242,"started_at":1761264000000}"#;

#[test]
fn the_probe_answer_gives_the_root_the_record_and_the_profiles_of_the_remote_host() {
    let text = format!("/srv/state\n{RECORD}\nprofile-mail\nprofile-chat\n");
    let host = parse_probe(&text).expect("a running host");
    assert_eq!(host.root, std::path::PathBuf::from("/srv/state"));
    assert_eq!(host.host_instance, "7c1f8a9e-2b44-4f4c-9d3f-3c0b6f6f2a11");
    assert_eq!(host.profiles, vec!["mail".to_string(), "chat".to_string()]);
}

#[test]
fn a_record_that_no_running_host_wrote_is_not_a_host() {
    assert!(parse_probe("").is_none(), "an empty answer");
    assert!(parse_probe("/srv/state\n").is_none(), "no record");
    assert!(
        parse_probe("/srv/state\n{\"host_instance\":\"\",\"pid\":1}\n").is_none(),
        "a blank instance"
    );
    assert!(
        parse_probe("\n{\"host_instance\":\"instance\",\"pid\":1}\n").is_none(),
        "no root"
    );
    assert!(
        parse_probe(&format!("/srv/state\n{RECORD}\n")).is_some(),
        "a host with no profile yet is still a host"
    );
}

#[test]
fn a_profile_that_the_remote_host_does_not_serve_names_the_reason() {
    let error = RemoteBridgeError::ProfileUnavailable {
        target: "browser-vm".to_string(),
        profile: "mail".to_string(),
        reason: "its host file names [chat]".to_string(),
    };
    let message = error.to_string();
    assert!(message.contains("browser-vm"), "{message}");
    assert!(message.contains("mail"), "{message}");
    assert!(
        message.contains("chat"),
        "the answer names what the host serves: {message}"
    );
}

#[test]
fn a_missing_actor_is_a_typed_refusal() {
    let message = RemoteBridgeError::ActorUnavailable.to_string();
    assert!(message.contains("HORIZON_BROWSER_ACTOR"), "{message}");
    assert!(
        message.contains("remote browser host unavailable"),
        "the reason is listed for the person: {message}"
    );
}

#[test]
fn a_finished_capture_adds_its_bytes_and_keeps_the_path_of_the_remote_host() {
    let result = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "result": {
            "content": [{"type": "text", "text": "{\"path\":\"/srv/state/captures/record.webm\"}"}],
            "structuredContent": {"panel_id": "profile-mail-1", "path": "/srv/state/captures/record.webm"}
        }
    });
    let mut message: Value = result;
    let capture = super::relay::capture_path(&message, "video/webm").expect("a capture result");
    super::relay::add_capture(&mut message, &capture, b"webm").expect("an added block");
    assert_eq!(
        message["result"]["structuredContent"]["path"],
        json!("/srv/state/captures/record.webm"),
        "the private remote path stays in the structured result"
    );
    let block = &message["result"]["content"][1];
    assert_eq!(block["type"], json!("resource"));
    assert_eq!(block["resource"]["mimeType"], json!("video/webm"));
    assert_eq!(
        block["resource"]["blob"],
        json!("d2VibQ=="),
        "the bytes cross base64 encoded"
    );
    assert!(
        block["resource"]["uri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with("horizon-browser://capture/")),
        "{block}"
    );
}
