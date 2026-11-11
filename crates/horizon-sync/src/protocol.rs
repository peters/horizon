//! The frames of a link, and the rules that a device name and a frame follow.
//!
//! A link is one connection of one computer. The first frame is a [`Hello`] and
//! the answer is a [`HelloOk`]; after that the computer pushes intents and pulls
//! changes, and it may wait for one. A version is one integer: the server refuses
//! a computer that speaks a higher one and answers the one that it speaks.

use serde::{Deserialize, Serialize};

use crate::intent::{Intent, PushReply};
pub use crate::store::{Changes, Row};

/// The version of the frames that this Horizon speaks.
///
/// Version 1 is the machine list of a server. Version 2 adds the secrets, which
/// are one kind of row, and the flag of the hello that says that a server cannot
/// read the secrets that it holds. A version 1 computer does not read either, so
/// the two sides of one link speak the same version.
pub const PROTOCOL_VERSION: u32 = 2;

/// The longest device name, and the characters of one: `a-z`, `0-9` and `-`.
const DEVICE_NAME_CHARS: usize = 40;

/// The first frame of a link.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// The version of the frames that the computer speaks.
    pub protocol: u32,
    /// The name of the computer, which the transport does not prove.
    pub device: String,
    /// The store id that the computer saw last, when it saw one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// The answer of the server to a [`Hello`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelloOk {
    /// The version of the frames that the server speaks.
    pub protocol: u32,
    /// The identifier of the server store, which every tip names.
    pub store: String,
    /// The time of the server, in milliseconds since the Unix epoch.
    pub now_millis: u64,
    /// Whether a live secret of the server is sealed with a key that the server
    /// does not have, so this server cannot read it. A computer that still holds
    /// a secret then sends it again, and the value is new to this server.
    #[serde(default)]
    pub secrets_lost: bool,
}

/// The intents of one computer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Push {
    pub intents: Vec<Intent>,
}

/// A pull from an index, with the tip that names the log at that index.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pull {
    pub since: u64,
    /// The tip of the computer at `since`; an empty tip is a computer whose
    /// mirror holds no row yet.
    pub tip: String,
}

/// A pull that stays open until a change after `since`, or until the bound of the
/// server passes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    pub since: u64,
}

/// One frame of a link.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Frame {
    Hello(Hello),
    HelloOk(HelloOk),
    Push(Push),
    Pushed(PushReply),
    Pull(Pull),
    Changes(Changes),
    Wait(Wait),
    Error(LinkError),
}

/// An error of the link itself, as opposed to a refused intent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkError {
    pub code: LinkErrorCode,
    pub message: String,
}

impl LinkError {
    /// One error frame.
    #[must_use]
    pub fn new(code: LinkErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.to_owned(),
        }
    }
}

/// The typed codes of a link error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkErrorCode {
    /// A frame is longer than the maximum of the protocol.
    FrameTooLarge,
    /// A frame is not one document that this protocol reads.
    BadFrame,
    /// The computer speaks a higher protocol version.
    ProtocolVersion,
    /// The device name breaks the rule.
    BadDevice,
    /// A frame arrived before the hello, or a second hello did.
    NotHello,
    /// One push holds more intents than the maximum.
    TooManyIntents,
    /// The link holds more open waits than the maximum.
    TooManyWaits,
    /// The server store could not answer now; the same frame can be sent again.
    StoreFailed,
}

impl LinkErrorCode {
    /// The stable code of this error, for a log line or a caller.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FrameTooLarge => "frame_too_large",
            Self::BadFrame => "bad_frame",
            Self::ProtocolVersion => "protocol_version",
            Self::BadDevice => "bad_device",
            Self::NotHello => "not_hello",
            Self::TooManyIntents => "too_many_intents",
            Self::TooManyWaits => "too_many_waits",
            Self::StoreFailed => "store_failed",
        }
    }
}

/// Whether `name` is one device name: 1 to 40 characters of `a-z`, `0-9` and
/// `-`, with no `-` at either end.
#[must_use]
pub fn check_device_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= DEVICE_NAME_CHARS
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_is_a_lower_case_label() {
        for name in ["mbp-7c1", "a", "0", "computer-1"] {
            assert!(check_device_name(name), "{name} is one device name");
        }
        for name in ["", "MBP", "mbp_7", "-mbp", "mbp-", "mbp 7", &"a".repeat(41)] {
            assert!(!check_device_name(name), "{name:?} is not one device name");
        }
    }

    #[test]
    fn every_frame_round_trips_through_its_json() {
        let store = Changes {
            index: 3,
            reset: false,
            rows: vec![Row {
                kind: crate::KIND_MACHINE.to_owned(),
                id: "build-1".to_owned(),
                body: serde_json::json!({ "name": "build-1" }),
                version: 3,
                deleted: false,
            }],
        };
        let frames = vec![
            Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: "mbp-7c1".to_owned(),
                store: None,
            }),
            Frame::HelloOk(HelloOk {
                protocol: PROTOCOL_VERSION,
                store: "store-a".to_owned(),
                now_millis: 1_760_000_000_000,
                secrets_lost: true,
            }),
            Frame::Push(Push {
                intents: vec![Intent {
                    device: "mbp-7c1".to_owned(),
                    id: 1,
                    seen: Some(2),
                    op: crate::Op::MachineRemove {
                        name: "build-1".to_owned(),
                    },
                }],
            }),
            Frame::Pushed(PushReply {
                results: Vec::new(),
                next_expected: 2,
            }),
            Frame::Pull(Pull {
                since: 3,
                tip: "abc".to_owned(),
            }),
            Frame::Changes(store),
            Frame::Wait(Wait { since: 3 }),
            Frame::Error(LinkError::new(LinkErrorCode::BadFrame, "not one frame")),
        ];
        for frame in frames {
            let json = serde_json::to_string(&frame).expect("a frame is JSON");
            assert_eq!(
                serde_json::from_str::<Frame>(&json).expect("the frame reads back"),
                frame,
                "{json}"
            );
        }
    }

    #[test]
    fn a_frame_is_one_named_field_so_a_reader_knows_what_it_holds() {
        let frame = Frame::Wait(Wait { since: 4 });
        assert_eq!(
            serde_json::to_string(&frame).expect("a frame is JSON"),
            r#"{"wait":{"since":4}}"#
        );
        let error = Frame::Error(LinkError::new(LinkErrorCode::TooManyWaits, "at most 32"));
        assert_eq!(
            serde_json::to_string(&error).expect("a frame is JSON"),
            r#"{"error":{"code":"too_many_waits","message":"at most 32"}}"#
        );
        assert!(
            serde_json::from_str::<Frame>(r#"{"wait":{"since":1},"pull":{"since":1}}"#).is_err(),
            "a frame holds one message"
        );
    }
}
