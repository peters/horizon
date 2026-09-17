use serde::{Deserialize, Serialize};

/// Connection details are separate from the device identity and controller OS.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Endpoint {
    LocalX11 { display: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub endpoint: Endpoint,
}

/// Coordinates are relative to this observed surface, in screenshot pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub target_id: String,
    pub surface_id: String,
    pub width: u32,
    pub height: u32,
    pub revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Space,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Modifier {
    Control,
    Shift,
    Alt,
    Meta,
}

/// Input units stay explicit. A wheel notch is not a mobile swipe or pixel scroll.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Action {
    Click {
        at: Point,
        button: Button,
    },
    Drag {
        from: Point,
        to: Point,
        duration_ms: u32,
    },
    Scroll {
        at: Point,
        vertical_notches: i32,
        horizontal_notches: i32,
    },
    Type {
        text: String,
    },
    Key {
        key: Key,
        modifiers: Vec<Modifier>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ActRequest {
    pub geometry: Geometry,
    pub action: Action,
}

/// Capabilities are advertised only when implemented by the active backend.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Capability {
    Screenshot,
    Pointer,
    Keyboard,
    Touch,
    Accessibility,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Observation {
    pub geometry: Geometry,
    pub captured_unix_ms: u128,
    pub mime_type: String,
    pub image_base64: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Readiness {
    pub geometry: Geometry,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActionReceipt {
    /// Input reached the backend; the caller must observe the application effect.
    pub state: String,
    pub geometry: Geometry,
}
