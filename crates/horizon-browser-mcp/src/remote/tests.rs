//! These tests use the JSON of a real tool result rather than a live machine. The
//! record of a host it did not start, and the command line of the remote server,
//! belong to the channel that the bridge shares with a browser panel, so
//! `horizon_browser_control::remote_host` tests them.

use serde_json::{Value, json};

use super::RemoteBridgeError;

#[test]
fn a_profile_that_the_host_does_not_serve_reaches_it_and_is_refused_there() {
    let relay = super::relay::Relay::new("browser-vm".to_string(), std::path::PathBuf::from("/srv/state"), None);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {"name": "browser_create", "arguments": {"target": "mail", "url": "https://example.com"}}
    })
    .to_string();
    assert_eq!(
        relay.request(request.as_bytes()),
        request.as_bytes(),
        "a bridge without a profile of its own forwards the profile of the call"
    );
    let refusal = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "result": {
            "content": [{"type": "text", "text":
                "browser action create-1 failed (target_unknown): profile `mail` is not in the host file"}],
            "isError": true
        }
    })
    .to_string();
    assert_eq!(
        relay.response(refusal.as_bytes()),
        refusal.as_bytes(),
        "the reason of the host reaches the caller unchanged"
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
