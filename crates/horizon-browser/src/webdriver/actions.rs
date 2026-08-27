use serde_json::{Value, json};

use crate::{BrowserButton, BrowserInput, BrowserKey, BrowserModifiers};

#[derive(Default)]
pub(super) struct ActionState {
    modifiers: BrowserModifiers,
}

impl ActionState {
    pub(super) fn payload(&mut self, input: BrowserInput) -> Value {
        let mut sources = Vec::new();
        match input {
            BrowserInput::MouseMove { x, y, modifiers, .. } => {
                self.push_modifier_source(modifiers, &mut sources);
                sources.push(pointer_source(vec![pointer_move(x, y)]));
            }
            BrowserInput::MousePress {
                x,
                y,
                button,
                modifiers,
                ..
            } => {
                self.push_modifier_source(modifiers, &mut sources);
                sources.push(pointer_source(vec![
                    pointer_move(x, y),
                    json!({ "type": "pointerDown", "button": button_number(button) }),
                ]));
            }
            BrowserInput::MouseRelease {
                x,
                y,
                button,
                modifiers,
                ..
            } => {
                sources.push(pointer_source(vec![
                    pointer_move(x, y),
                    json!({ "type": "pointerUp", "button": button_number(button) }),
                ]));
                self.push_modifier_source(modifiers, &mut sources);
            }
            BrowserInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            } => {
                self.push_modifier_source(modifiers, &mut sources);
                sources.push(json!({
                    "type": "wheel",
                    "id": "horizon-wheel",
                    "actions": [{
                        "type": "scroll",
                        "duration": 0,
                        "origin": "viewport",
                        "x": finite_i64(x),
                        "y": finite_i64(y),
                        "deltaX": finite_i64(delta_x),
                        "deltaY": finite_i64(delta_y),
                    }]
                }));
            }
            BrowserInput::KeyDown {
                key, text, modifiers, ..
            } => {
                let mut actions = modifier_transitions(self.modifiers, modifiers);
                actions.push(json!({ "type": "keyDown", "value": key_value(key, text.as_deref()) }));
                self.modifiers = modifiers;
                sources.push(key_source(actions));
            }
            BrowserInput::KeyUp {
                key, text, modifiers, ..
            } => {
                let mut actions = vec![json!({ "type": "keyUp", "value": key_value(key, text.as_deref()) })];
                actions.extend(modifier_transitions(self.modifiers, modifiers));
                self.modifiers = modifiers;
                sources.push(key_source(actions));
            }
            BrowserInput::InsertText { text } => {
                let mut actions = modifier_transitions(self.modifiers, BrowserModifiers::none());
                self.modifiers = BrowserModifiers::none();
                actions.extend(text.chars().flat_map(|character| {
                    let value = character.to_string();
                    [
                        json!({ "type": "keyDown", "value": value }),
                        json!({ "type": "keyUp", "value": value }),
                    ]
                }));
                sources.push(key_source(actions));
            }
        }
        json!({ "actions": sources })
    }

    pub(super) fn click_payload(
        &mut self,
        x: f64,
        y: f64,
        button: BrowserButton,
        count: u32,
        modifiers: BrowserModifiers,
    ) -> Value {
        let mut sources = Vec::new();
        self.push_modifier_source(modifiers, &mut sources);
        let mut actions = vec![pointer_move(x, y)];
        for _ in 0..count {
            actions.push(json!({ "type": "pointerDown", "button": button_number(button) }));
            actions.push(json!({ "type": "pointerUp", "button": button_number(button) }));
        }
        sources.push(pointer_source(actions));
        json!({ "actions": sources })
    }

    fn push_modifier_source(&mut self, modifiers: BrowserModifiers, sources: &mut Vec<Value>) {
        let actions = modifier_transitions(self.modifiers, modifiers);
        self.modifiers = modifiers;
        if !actions.is_empty() {
            sources.push(key_source(actions));
        }
    }
}

fn pointer_source(actions: Vec<Value>) -> Value {
    let actions = Value::Array(actions);
    json!({
        "type": "pointer",
        "id": "horizon-pointer",
        "parameters": { "pointerType": "mouse" },
        "actions": actions,
    })
}

fn pointer_move(x: f64, y: f64) -> Value {
    json!({
        "type": "pointerMove",
        "duration": 0,
        "origin": "viewport",
        "x": finite_i64(x),
        "y": finite_i64(y),
    })
}

fn key_source(actions: Vec<Value>) -> Value {
    let actions = Value::Array(actions);
    json!({ "type": "key", "id": "horizon-keyboard", "actions": actions })
}

fn modifier_transitions(from: BrowserModifiers, to: BrowserModifiers) -> Vec<Value> {
    let mut actions = Vec::new();
    for (was_down, is_down, value) in [
        (from.alt, to.alt, "\u{e00a}"),
        (from.ctrl, to.ctrl, "\u{e009}"),
        (from.meta, to.meta, "\u{e03d}"),
        (from.shift, to.shift, "\u{e008}"),
    ] {
        match (was_down, is_down) {
            (false, true) => actions.push(json!({ "type": "keyDown", "value": value })),
            (true, false) => actions.push(json!({ "type": "keyUp", "value": value })),
            _ => {}
        }
    }
    actions
}

fn key_value(key: BrowserKey, text: Option<&str>) -> String {
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        return text.to_string();
    }
    match key {
        BrowserKey::ArrowUp => "\u{e013}",
        BrowserKey::ArrowDown => "\u{e015}",
        BrowserKey::ArrowLeft => "\u{e012}",
        BrowserKey::ArrowRight => "\u{e014}",
        BrowserKey::Enter => "\u{e007}",
        BrowserKey::Tab => "\u{e004}",
        BrowserKey::Escape => "\u{e00c}",
        BrowserKey::Space => " ",
        BrowserKey::Backspace => "\u{e003}",
        BrowserKey::Delete => "\u{e017}",
        BrowserKey::Home => "\u{e011}",
        BrowserKey::End => "\u{e010}",
        BrowserKey::PageUp => "\u{e00e}",
        BrowserKey::PageDown => "\u{e00f}",
        BrowserKey::Insert => "\u{e016}",
        BrowserKey::F1 => "\u{e031}",
        BrowserKey::F2 => "\u{e032}",
        BrowserKey::F3 => "\u{e033}",
        BrowserKey::F4 => "\u{e034}",
        BrowserKey::F5 => "\u{e035}",
        BrowserKey::F6 => "\u{e036}",
        BrowserKey::F7 => "\u{e037}",
        BrowserKey::F8 => "\u{e038}",
        BrowserKey::F9 => "\u{e039}",
        BrowserKey::F10 => "\u{e03a}",
        BrowserKey::F11 => "\u{e03b}",
        BrowserKey::F12 => "\u{e03c}",
        BrowserKey::F13 => "\u{e040}",
        BrowserKey::F14 => "\u{e041}",
        BrowserKey::F15 => "\u{e042}",
        BrowserKey::Char(character) => return character.to_string(),
    }
    .to_string()
}

const fn button_number(button: BrowserButton) -> u8 {
    match button {
        BrowserButton::Left => 0,
        BrowserButton::Middle => 1,
        BrowserButton::Right => 2,
    }
}

fn finite_i64(value: f64) -> i64 {
    if value.is_finite() {
        finite_i64_from_f64(value)
    } else {
        0
    }
}

#[allow(clippy::cast_possible_truncation)]
fn finite_i64_from_f64(value: f64) -> i64 {
    // Rust's float-to-integer conversion saturates outside the destination
    // range. Rounding first preserves WebDriver's required integer shape.
    value.round() as i64
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::ActionState;
    use crate::{BrowserButton, BrowserInput, BrowserKey, BrowserModifiers};

    #[test]
    fn printable_key_uses_w3c_key_source_and_modifier_transition() {
        let payload = ActionState::default().payload(BrowserInput::KeyDown {
            physical_key: Some(BrowserKey::Char('a')),
            key: BrowserKey::Char('A'),
            text: Some("A".to_string()),
            modifiers: BrowserModifiers {
                shift: true,
                ..BrowserModifiers::none()
            },
            repeat: false,
            edit_command: None,
        });
        assert_eq!(payload["actions"][0]["actions"][0]["value"], "\u{e008}");
        assert_eq!(payload["actions"][0]["actions"][1]["value"], "A");
    }

    #[test]
    fn raw_text_becomes_balanced_key_actions() {
        let mut state = ActionState::default();
        state.modifiers.meta = true;
        let payload = state.payload(BrowserInput::InsertText { text: "ab".to_string() });
        let actions = payload["actions"][0]["actions"].as_array();
        assert_eq!(actions.map(Vec::len), Some(5));
        assert_eq!(
            actions.and_then(|actions| actions[0]["value"].as_str()),
            Some("\u{e03d}")
        );
    }

    #[test]
    fn multiple_clicks_share_one_pointer_action_chain() {
        let payload =
            ActionState::default().click_payload(12.0, 24.0, BrowserButton::Left, 2, BrowserModifiers::none());
        let actions = payload["actions"][0]["actions"].as_array();

        assert_eq!(payload["actions"].as_array().map(Vec::len), Some(1));
        assert_eq!(actions.map(Vec::len), Some(5));
        assert_eq!(
            actions
                .and_then(|actions| actions.get(1))
                .and_then(|action| action.get("type"))
                .and_then(Value::as_str),
            Some("pointerDown")
        );
        assert_eq!(
            actions
                .and_then(|actions| actions.get(4))
                .and_then(|action| action.get("type"))
                .and_then(Value::as_str),
            Some("pointerUp")
        );
    }
}
