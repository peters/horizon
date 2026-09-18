use super::{Scripted, json, send_keys_through};

#[test]
fn fill_requires_a_positive_live_value_assertion() {
    for retained in [json!(false), json!(null), json!("true"), json!({})] {
        let transport = Scripted::new(vec![
            Ok(json!({"value": {"ELEMENT": "field"}})),
            Ok(json!({"value": null})),
            Ok(json!({"value": null})),
            Ok(json!({"value": retained})),
        ]);
        let error = send_keys_through(&transport, "/session/s1", "#date", "2030-01-15T12:37")
            .expect_err("transport success alone cannot confirm fill");
        assert!(error.contains("did not retain"));
        assert!(!error.contains("2030"), "field contents stay private");
        let sent = transport.sent();
        assert_eq!(sent[3].1, "/session/s1/execute/sync");
        assert_eq!(sent[3].2["args"], json!(["#date", "2030-01-15T12:37"]));
    }
}

#[test]
fn clearing_verifies_empty_without_sending_empty_keys() {
    let transport = Scripted::new(vec![
        Ok(json!({"value": {"ELEMENT": "field"}})),
        Ok(json!({"value": null})),
        Ok(json!({"value": true})),
    ]);
    send_keys_through(&transport, "/session/s1", "#date", "").expect("verified clear");
    let sent = transport.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1].1, "/session/s1/element/field/clear");
    assert_eq!(sent[2].1, "/session/s1/execute/sync");
    assert_eq!(sent[2].2["args"], json!(["#date", ""]));
}

#[test]
fn failed_postcondition_read_is_a_failed_fill() {
    let transport = Scripted::new(vec![
        Ok(json!({"value": {"ELEMENT": "field"}})),
        Ok(json!({"value": null})),
        Ok(json!({"value": null})),
        Err("navigation interrupted the read".into()),
    ]);
    assert!(send_keys_through(&transport, "/session/s1", "#text", "hello").is_err());
}
