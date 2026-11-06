use horizon_device::{ActRequest, Action};
use serde_json::{Value, json};

const LETTERS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w",
    "x", "y", "z",
];
const NAMED: [&str; 12] = [
    "enter",
    "escape",
    "tab",
    "backspace",
    "delete",
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "space",
];

fn geometry() -> Value {
    json!({
        "target_id": "fixture",
        "surface_id": "view",
        "width": 1280,
        "height": 800,
        "revision": "one"
    })
}

fn request(action: Value) -> Value {
    json!({"geometry": geometry(), "action": action})
}

fn parse(action: Value) -> Result<ActRequest, serde_json::Error> {
    serde_json::from_value(request(action))
}

fn key_action(key: &str, modifiers: &[&str]) -> Value {
    json!({"kind": "key", "key": key, "modifiers": modifiers})
}

fn assert_key_round_trip(key: &str, modifiers: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let original = request(key_action(key, modifiers));
    let parsed = serde_json::from_value::<ActRequest>(original.clone())?;
    assert!(
        matches!(&parsed.action, Action::Key { .. }),
        "key {key:?} must deserialize as Action::Key"
    );
    let encoded = serde_json::to_value(&parsed)?;
    assert_eq!(
        encoded, original,
        "key {key:?} must round-trip to the same JSON key string and modifier order"
    );
    assert_eq!(
        encoded["action"]["key"],
        json!(key),
        "key {key:?} must serialize in the lowercase string form"
    );
    assert_eq!(
        encoded["action"]["modifiers"],
        json!(modifiers),
        "key {key:?} must preserve modifier order"
    );
    Ok(())
}

#[test]
fn lowercase_letter_chords_parse_and_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    for key in LETTERS {
        assert_key_round_trip(key, &["control"])?;
    }
    Ok(())
}

#[test]
fn control_shift_letter_chord_preserves_modifier_order() -> Result<(), Box<dyn std::error::Error>> {
    assert_key_round_trip("p", &["control", "shift"])?;
    assert_key_round_trip("p", &["shift", "control"])?;
    let first = serde_json::to_value(parse(key_action("p", &["control", "shift"]))?)?;
    let second = serde_json::to_value(parse(key_action("p", &["shift", "control"]))?)?;
    assert_eq!(first["action"]["key"], json!("p"));
    assert_eq!(first["action"]["modifiers"], json!(["control", "shift"]));
    assert_eq!(second["action"]["modifiers"], json!(["shift", "control"]));
    Ok(())
}

#[test]
fn named_keys_still_parse_and_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    for key in NAMED {
        assert_key_round_trip(key, &[])?;
        assert_key_round_trip(key, &["control", "shift", "alt", "meta"])?;
    }
    Ok(())
}

#[test]
fn unknown_key_names_and_uppercase_aliases_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    for key in ["quux", "qq", "return", "escapex", "Q", "A", "Z", "control"] {
        let result = serde_json::from_value::<ActRequest>(request(key_action(key, &["control"])));
        assert!(
            result.is_err(),
            "unspecified key name {key:?} must be rejected, not accepted"
        );
    }
    Ok(())
}

#[test]
fn geometry_and_unknown_field_guards_are_unchanged() -> Result<(), Box<dyn std::error::Error>> {
    // A named key keeps malformed geometry and unknown fields the only failures.
    let unknown_field = json!({
        "geometry": geometry(),
        "action": key_action("enter", &["control"]),
        "repeat": 2
    });
    assert!(serde_json::from_value::<ActRequest>(unknown_field).is_err());

    let mut action = key_action("enter", &["control"]);
    action["repeat"] = json!(2);
    assert!(serde_json::from_value::<ActRequest>(request(action)).is_err());

    for broken in [
        json!({"target_id": "fixture", "surface_id": "view", "width": 1280, "revision": "one"}),
        json!({"target_id": "fixture", "surface_id": "view", "width": -1, "height": 800, "revision": "one"}),
        json!({"target_id": "fixture", "surface_id": "view", "width": 1280, "height": 800, "revision": "one", "extra": true}),
    ] {
        let guarded = json!({"geometry": broken, "action": key_action("enter", &[])});
        assert!(serde_json::from_value::<ActRequest>(guarded).is_err());
    }
    Ok(())
}
