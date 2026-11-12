//! The input of a person, as the `input` messages of the contract.
//!
//! A point is a pixel of the picture that the viewer shows, and never a pixel of
//! the panel: the widget maps its own pointer through the crop and scale of the
//! image before it builds one of these events. The program adds no modifier of its
//! own, so a chord arrives as the modifier keys of [`crate::keys::NamedKey`] and
//! then the key of the chord.
use serde_json::{Value, json};

use crate::keys::{Button, Key};

/// One point of the picture that the viewer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

/// One input event of a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// The pointer is at a pixel of the picture.
    Pointer { at: Point },
    /// A button went down or up, at the pointer or at a point it names.
    Button {
        button: Button,
        down: bool,
        at: Option<Point>,
    },
    /// One step of the wheel: a positive `dy` is down and a positive `dx` is
    /// right.
    Wheel { dx: i32, dy: i32, at: Option<Point> },
    /// A named key or one character, down or up.
    Key { key: Key, down: bool },
    /// Text that a person typed, as one message, so a character is right with two
    /// keyboard layouts.
    Text { text: String },
}

impl InputEvent {
    /// The body of this event as one `input` message.
    #[must_use]
    pub fn body(&self) -> Value {
        let mut object = match self {
            Self::Pointer { at } => json!({ "type": "input", "event": "pointer", "x": at.x, "y": at.y }),
            Self::Button { button, down, at } => {
                let mut object = json!({
                    "type": "input",
                    "event": "button",
                    "button": button.name(),
                    "down": down,
                });
                add_point(&mut object, *at);
                object
            }
            Self::Wheel { dx, dy, at } => {
                let mut object = json!({ "type": "input", "event": "wheel", "dx": dx, "dy": dy });
                add_point(&mut object, *at);
                object
            }
            Self::Key { key, down } => json!({
                "type": "input",
                "event": "key",
                "key": key.name(),
                "down": down,
            }),
            Self::Text { text } => json!({ "type": "input", "event": "text", "text": text }),
        };
        if let Value::Object(map) = &mut object {
            // A member is absent rather than null, so a message holds the members
            // that its event has and no other.
            map.retain(|_, value| !value.is_null());
        }
        object
    }
}

fn add_point(object: &mut Value, at: Option<Point>) {
    if let (Some(at), Value::Object(map)) = (at, object) {
        map.insert("x".into(), at.x.into());
        map.insert("y".into(), at.y.into());
    }
}

/// What the viewer has sent, so one pointer position and one button mask are sent
/// as their changes and not once for each event of a frame.
#[derive(Debug, Default)]
pub struct InputState {
    position: Option<Point>,
    buttons: u8,
}

impl InputState {
    /// The RFB mask of the buttons that the program believes are down.
    #[must_use]
    pub fn buttons(&self) -> u8 {
        self.buttons
    }

    /// The last point that the program was given.
    #[must_use]
    pub fn position(&self) -> Option<Point> {
        self.position
    }

    /// A pointer move to a point, or to no point when it left the image. A message
    /// goes only when the position changed, or when the pointer re-enters.
    pub fn pointer(&mut self, at: Option<Point>) -> Vec<InputEvent> {
        let Some(at) = at else {
            return Vec::new();
        };
        if self.position == Some(at) {
            return Vec::new();
        }
        self.position = Some(at);
        vec![InputEvent::Pointer { at }]
    }

    /// A button of the pointer, with the mask of the buttons that are still down.
    pub fn button(&mut self, button: Button, down: bool, at: Option<Point>) -> Vec<InputEvent> {
        let bit = mask_of(button);
        let before = self.buttons;
        self.buttons = if down {
            self.buttons | bit
        } else {
            self.buttons & !bit
        };
        if before == self.buttons {
            return Vec::new();
        }
        if let Some(at) = at {
            self.position = Some(at);
        }
        vec![InputEvent::Button {
            button,
            down,
            at: Some(self.position.unwrap_or(Point { x: 0, y: 0 })),
        }]
    }

    /// One step of the wheel at the pointer.
    #[must_use]
    pub fn wheel(&self, dx: i32, dy: i32) -> Vec<InputEvent> {
        if dx == 0 && dy == 0 {
            return Vec::new();
        }
        vec![InputEvent::Wheel { dx, dy, at: self.position }]
    }

    /// A key that a person pressed or released.
    #[must_use]
    pub fn key(&mut self, key: Key, down: bool) -> Vec<InputEvent> {
        vec![InputEvent::Key { key, down }]
    }

    /// Text that a person typed: one message for the whole text of this frame, and
    /// no control character.
    #[must_use]
    pub fn text(text: &str) -> Vec<InputEvent> {
        let text: String = text.chars().filter(|character| !character.is_control()).collect();
        if text.is_empty() {
            return Vec::new();
        }
        vec![InputEvent::Text { text }]
    }
}

const fn mask_of(button: Button) -> u8 {
    match button {
        Button::Left => horizon_vnc::BUTTON_LEFT,
        Button::Middle => horizon_vnc::BUTTON_MIDDLE,
        Button::Right => horizon_vnc::BUTTON_RIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_event_is_the_message_of_its_own_shape() {
        assert_eq!(
            InputEvent::Pointer {
                at: Point { x: 10, y: 20 }
            }
            .body(),
            json!({"type": "input", "event": "pointer", "x": 10, "y": 20})
        );
        assert_eq!(
            InputEvent::Button {
                button: Button::Middle,
                down: true,
                at: Some(Point { x: 1, y: 2 })
            }
            .body(),
            json!({"type": "input", "event": "button", "button": "middle", "down": true, "x": 1, "y": 2})
        );
        assert_eq!(
            InputEvent::Wheel {
                dx: -1,
                dy: 2,
                at: Some(Point { x: 3, y: 4 })
            }
            .body(),
            json!({"type": "input", "event": "wheel", "dx": -1, "dy": 2, "x": 3, "y": 4})
        );
        assert_eq!(
            InputEvent::Key {
                key: Key::Character('a'),
                down: true
            }
            .body(),
            json!({"type": "input", "event": "key", "key": "a", "down": true})
        );
        assert_eq!(
            InputEvent::Text { text: "hei".into() }.body(),
            json!({"type": "input", "event": "text", "text": "hei"})
        );
    }

    #[test]
    fn a_pointer_and_a_button_are_sent_as_their_changes() {
        let mut state = InputState::default();
        assert_eq!(
            state.pointer(Some(Point { x: 4, y: 5 })),
            [InputEvent::Pointer {
                at: Point { x: 4, y: 5 }
            }]
        );
        assert!(state.pointer(Some(Point { x: 4, y: 5 })).is_empty(), "one position is one message");
        assert_eq!(state.buttons(), 0);
        let events = state.button(Button::Left, true, Some(Point { x: 6, y: 7 }));
        assert_eq!(
            events,
            [InputEvent::Button {
                button: Button::Left,
                down: true,
                at: Some(Point { x: 6, y: 7 })
            }]
        );
        assert_eq!(state.buttons(), horizon_vnc::BUTTON_LEFT);
        assert!(state.button(Button::Left, true, None).is_empty(), "a held button is no news");
        assert_eq!(state.wheel(0, 0), Vec::new(), "no travel is no message");
        assert_eq!(
            state.wheel(0, 1),
            [InputEvent::Wheel {
                dx: 0,
                dy: 1,
                at: Some(Point { x: 6, y: 7 })
            }]
        );
        state.button(Button::Left, false, None);
        assert_eq!(state.buttons(), 0);
    }

    #[test]
    fn typed_text_loses_every_control_character_and_nothing_else() {
        assert_eq!(
            InputState::text("he\ti\n"),
            [InputEvent::Text { text: "hei".into() }],
            "a tab and a newline are no character of a text"
        );
        assert_eq!(InputState::text("\u{7}"), Vec::new());
        assert_eq!(
            InputState::text("ø"),
            [InputEvent::Text { text: "ø".into() }],
            "a character of two bytes travels as it is"
        );
    }
}
