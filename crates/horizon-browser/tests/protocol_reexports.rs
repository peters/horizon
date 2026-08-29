use horizon_browser::{
    AgentActionResult, BackendKind, BrowserActionOutcome, BrowserAuditAction, BrowserButton, BrowserControlAction,
    BrowserControlValue, BrowserInput, BrowserModifiers, BrowserNetworkCapture, BrowserTarget,
};

#[test]
fn engine_reexports_are_protocol_types_with_stable_json() {
    let action = BrowserControlAction::Input {
        input: BrowserInput::MousePress {
            x: 12.0,
            y: 18.0,
            button: BrowserButton::Left,
            click_count: 2,
            buttons: 1,
            modifiers: BrowserModifiers::none(),
        },
    };
    let action_json = serde_json::to_value(&action).unwrap_or_default();
    let decoded_action = serde_json::from_value::<horizon_browser_protocol::BrowserControlAction>(action_json.clone())
        .unwrap_or(BrowserControlAction::Reload);

    assert_eq!(decoded_action, action);
    assert_eq!(
        action_json,
        serde_json::json!({
            "type": "input",
            "input": {
                "type": "mouse_press",
                "x": 12.0,
                "y": 18.0,
                "button": "left",
                "click_count": 2,
                "buttons": 1,
                "modifiers": {
                    "alt": false,
                    "ctrl": false,
                    "meta": false,
                    "shift": false
                }
            }
        })
    );

    let result = AgentActionResult {
        schema_version: 1,
        action_id: "action-1".to_string(),
        completed_at_millis: 42,
        outcome: BrowserActionOutcome::Completed {
            value: BrowserControlValue::Network {
                capture: BrowserNetworkCapture {
                    capture_id: "capture-1".to_string(),
                    path: "/private/capture.ndjson".to_string(),
                    active: true,
                    transport: "web_driver_bidi".to_string(),
                    records_enqueued: 3,
                    records_written: 2,
                    bytes_written: 128,
                    records_dropped: 0,
                    payloads_truncated: 0,
                    file_limit_reached: false,
                    writer_failed: false,
                    known_connections: Vec::new(),
                    connections_truncated: 0,
                },
            },
        },
    };
    let result_json = serde_json::to_string(&result).unwrap_or_default();
    let decoded_result = serde_json::from_str::<horizon_browser_protocol::AgentActionResult>(&result_json)
        .unwrap_or_else(|_| AgentActionResult::failed("decode", horizon_browser::BrowserControlFailure::new("x", "x")));

    assert_eq!(decoded_result, result);

    let audit = BrowserAuditAction::session_created(
        BackendKind::FirefoxBidi,
        Some("https://user:secret@example.test/path?token=secret"),
        false,
    );
    let audit_json = serde_json::to_string(&audit).unwrap_or_default();
    let decoded_audit = serde_json::from_str::<horizon_browser_protocol::BrowserAuditAction>(&audit_json)
        .unwrap_or(BrowserAuditAction::Reload);

    assert_eq!(decoded_audit, audit);
    assert!(!audit_json.contains("secret"));

    let target = BrowserTarget::Selector {
        selector: "#save".to_string(),
    };
    let protocol_target: horizon_browser_protocol::BrowserTarget = target.clone();
    assert_eq!(protocol_target, target);
}
