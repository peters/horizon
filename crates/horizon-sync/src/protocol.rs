//! The frames of a link, and the rules that a device name and a frame follow.
//!
//! A link is one connection of one computer. The first frame is a [`Hello`] and
//! the answer is a [`HelloOk`]; after that the computer pushes intents and pulls
//! changes, and it may wait for one. A version is one integer: the server refuses
//! a computer that speaks a higher one and answers the one that it speaks.
//!
//! The two numbers of a link are not the same one. [`PROTOCOL_VERSION`] is the
//! frames that a computer sends, and [`SERVER_PROTOCOL_VERSION`] is what a server
//! of this Horizon answers: version 5 changes no frame, only the time that a
//! server holds an idle wait, and version 6 adds the frames of a live link, which
//! a computer sends only to a server that answered 6. A computer reads the answer
//! of the greeting and keeps its own longer bound for a server of an older
//! version, so the two sides of a link may be of different versions without a
//! frame that either cannot read.

use serde::{Deserialize, Serialize};

use crate::intent::{Intent, PushReply};
use crate::live::{LiveEvent, LiveRequest};
pub use crate::store::{Changes, Row};

/// The version of the frames that this Horizon speaks.
///
/// Version 1 is the machine list of a server. Version 2 adds the secrets, which
/// are one kind of row, and the flag of the hello that says that a server cannot
/// read the secrets that it holds. Version 3 adds the login over the TCP listener
/// of a server: the `join` of a computer with the code of an invitation, the
/// token that a later `hello` carries, and the request that makes an invitation
/// and lists the computers. Version 4 adds the `setting` object, which carries one
/// section of the settings of the person: an older server would answer one intent
/// of that kind with `unknown_kind`, so the two sides of one link speak the same
/// version and an older server refuses this computer with `protocol_version`
/// instead of dropping its settings one by one.
pub const PROTOCOL_VERSION: u32 = 4;

/// The version that a server of this Horizon answers in its greeting.
///
/// Version 5 adds no frame. A server that answers 5 holds an idle wait open for
/// fifteen seconds instead of fifty, so a computer that reads 5 proves that its
/// link is alive within about twenty seconds. A computer of version 4 reads the
/// same answer at its own bound of sixty-five seconds and is unaffected, and a
/// computer of this build keeps that longer bound for a server that answers 4.
///
/// Version 6 adds the two frames of a live link. The hello of a computer stays at
/// [`PROTOCOL_VERSION`], so an older server still greets it, and the computer
/// sends a live frame only to a server that answered [`LIVE_PROTOCOL_VERSION`]:
/// an older server cannot read one and would end the link. Version 7 adds the
/// facts of a screen that say whether a follower keeps its history and who takes
/// the screen, and the mark of the replay of a screen. Version 8 adds the
/// heartbeat of a live link in both directions, so a computer that went away
/// leaves the list of the others.
pub const SERVER_PROTOCOL_VERSION: u32 = 8;

/// The first version of a server that holds an idle wait for fifteen seconds.
pub const SHORT_WAIT_PROTOCOL_VERSION: u32 = 5;

/// The first version of a server that relays the shared terminals of a live link,
/// which is [version 8](SERVER_PROTOCOL_VERSION) with the heartbeat of a link.
pub const LIVE_PROTOCOL_VERSION: u32 = 8;

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
    /// The token that a `join` gave this computer. It is present on a link of the
    /// TCP listener and absent on a link of the Unix socket, where `ssh` is the
    /// login. The server keeps the hash of a token and never the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// The first frame of a computer that joins the server with the code of an
/// invitation, on the TCP listener of a server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Join {
    /// The code of one invitation, in three groups of four or without dashes.
    pub code: String,
    /// The name that this computer takes on the server.
    pub device: String,
}

/// The answer to a [`Join`]: the token of this computer, which every later
/// `hello` of it carries.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Joined {
    /// The name that the server gave this computer.
    pub device: String,
    /// The token of this computer on the server.
    pub token: String,
}

/// The request of a logged-in computer for one invitation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invite {}

/// The answer to an [`Invite`]: one code, the link that holds it, and the time
/// that the invitation still lives.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invited {
    /// The code of the invitation, in three groups of four.
    pub code: String,
    /// The link that a person pastes into another computer,
    /// `horizon://<host>:<port>/join#<CODE>`.
    pub link: String,
    /// The milliseconds that the invitation still lives when the server answered.
    pub expires_millis: u64,
    /// The time of the server when it answered. A window compares it with the time
    /// that a computer joined, so it learns that the invitation was used.
    pub now_millis: u64,
}

/// The request for the list of the computers of this server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Devices {}

/// How one computer logs in on this server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLogin {
    /// A link of the Unix socket of the server, whose login is `ssh`.
    Ssh,
    /// A link of the TCP listener, whose login is the token of a join.
    Token,
}

impl DeviceLogin {
    /// The stable code of this login, for a row of the window and a tool answer.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Token => "token",
        }
    }
}

/// One computer of this server, as the list of the computers reports it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRow {
    /// The device name that the computer claimed, or that its token names.
    pub name: String,
    /// The time of the server when this computer first reached it.
    pub created_millis: u64,
    /// The time of the server when this computer last reached it.
    pub last_seen_millis: u64,
    /// How this computer logs in.
    pub login: DeviceLogin,
    /// Whether this row is the computer that asked for the list.
    pub caller: bool,
}

impl DeviceRow {
    /// Whether this row is the console of the server computer itself: the device
    /// that `horizon --server-invite` claims on its own server, so that a person who
    /// runs the server asks for the first link there.
    ///
    /// The server's store knows two things about a computer beyond its times: the
    /// name it claimed and the way it logs in. The console of the server claims the
    /// name [`SERVER_DEVICE`] over the `ssh` link of the server's own home, so both
    /// decide. A computer that a person named `server` and joined with a link logs
    /// in with its token, and a console of another computer holds the name of that
    /// computer, so neither is the server computer.
    ///
    /// The row of the server computer is not a computer that a person removes: it
    /// holds the login of the console of the server, and the server keeps running
    /// after the removal.
    #[must_use]
    pub fn is_server_console(&self) -> bool {
        self.login == DeviceLogin::Ssh && self.name == crate::invite::SERVER_DEVICE
    }
}

/// The list of the computers of this server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceList {
    /// The rows, in name order.
    pub rows: Vec<DeviceRow>,
    /// The time of the server when it answered, so a window can say how long ago
    /// it saw a computer without trusting its own clock against the server.
    pub now_millis: u64,
}

/// The request to remove one computer from this server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRemove {
    /// The name of the computer to remove.
    pub device: String,
}

/// The answer to a [`DeviceRemove`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRemoved {
    /// The name that the request named.
    pub device: String,
    /// Whether this server held that computer.
    pub removed: bool,
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
    Join(Join),
    Joined(Joined),
    Invite(Invite),
    Invited(Invited),
    Devices(Devices),
    DeviceList(DeviceList),
    DeviceRemove(DeviceRemove),
    DeviceRemoved(DeviceRemoved),
    Push(Push),
    Pushed(PushReply),
    Pull(Pull),
    Changes(Changes),
    Wait(Wait),
    Error(LinkError),
    /// One request of a live link, from a computer.
    Live(LiveRequest),
    /// One event of a live link, from the server.
    LiveEvent(LiveEvent),
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
    /// A computer asked for an invitation, and the server has no listener.
    NoListener,
    /// The code of a join is not one open invitation of this server.
    WrongCode,
    /// Too many wrong codes were tried, so every invitation is gone until a
    /// person makes a new one.
    JoinLocked,
    /// The device name of a join already holds a token on this server.
    NameTaken,
    /// The token of a link is not one of this server, or it belongs to another
    /// device.
    NotLoggedIn,
    /// This computer was removed from the server, and its link ends.
    Removed,
    /// A live request arrived on a link that is not a live link.
    NotLive,
    /// A frame that is not a live request arrived on a live link.
    LiveLink,
    /// Another live link of the same computer opened, so this one ends.
    LiveReplaced,
    /// The server holds the most live links, or this link did not read the frames
    /// that waited for it.
    LiveLimit,
    /// The computer of a live link sent nothing for thirty seconds, so the link
    /// ended. Its computer was gone without closing the transport.
    LiveSilent,
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
            Self::NoListener => "no_listener",
            Self::WrongCode => "wrong_code",
            Self::JoinLocked => "join_locked",
            Self::NameTaken => "name_taken",
            Self::NotLoggedIn => "not_logged_in",
            Self::Removed => "removed",
            Self::NotLive => "not_live",
            Self::LiveLink => "live_link",
            Self::LiveReplaced => "live_replaced",
            Self::LiveLimit => "live_limit",
            Self::LiveSilent => "live_silent",
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
    fn the_console_of_the_server_computer_is_told_by_its_name_and_its_login() {
        let row = |name: &str, login| DeviceRow {
            name: name.to_owned(),
            created_millis: 1,
            last_seen_millis: 1,
            login,
            caller: false,
        };
        assert!(
            row(crate::invite::SERVER_DEVICE, DeviceLogin::Ssh).is_server_console(),
            "the console of the server computer over ssh is the server itself"
        );
        assert!(
            !row(crate::invite::SERVER_DEVICE, DeviceLogin::Token).is_server_console(),
            "a computer that a person named server and joined with a token is a computer"
        );
        assert!(
            !row("mbp-7c1", DeviceLogin::Ssh).is_server_console(),
            "the console of another computer holds the name of that computer"
        );
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
                token: None,
            }),
            Frame::Hello(Hello {
                protocol: PROTOCOL_VERSION,
                device: "mbp-7c1".to_owned(),
                store: Some("store-a".to_owned()),
                token: Some("a-token-of-one-computer".to_owned()),
            }),
            Frame::Join(Join {
                code: "ABCD-EFGH-JKMN".to_owned(),
                device: "mbp-7c1".to_owned(),
            }),
            Frame::Joined(Joined {
                device: "mbp-7c1".to_owned(),
                token: "a-token-of-one-computer".to_owned(),
            }),
            Frame::Invite(Invite {}),
            Frame::Invited(Invited {
                code: "ABCD-EFGH-JKMN".to_owned(),
                link: "horizon://server-computer:7000/join#ABCD-EFGH-JKMN".to_owned(),
                expires_millis: 600_000,
                now_millis: 1_760_000_000_000,
            }),
            Frame::Devices(Devices {}),
            Frame::DeviceList(DeviceList {
                now_millis: 1_760_000_000_000,
                rows: vec![DeviceRow {
                    name: "mbp-7c1".to_owned(),
                    created_millis: 1_760_000_000_000,
                    last_seen_millis: 1_760_000_000_000,
                    login: DeviceLogin::Token,
                    caller: true,
                }],
            }),
            Frame::DeviceRemove(DeviceRemove {
                device: "mbp-7c1".to_owned(),
            }),
            Frame::DeviceRemoved(DeviceRemoved {
                device: "mbp-7c1".to_owned(),
                removed: true,
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
    fn every_code_of_a_link_that_ends_has_a_stable_name() {
        for (code, name) in [
            (LinkErrorCode::FrameTooLarge, "frame_too_large"),
            (LinkErrorCode::BadFrame, "bad_frame"),
            (LinkErrorCode::ProtocolVersion, "protocol_version"),
            (LinkErrorCode::BadDevice, "bad_device"),
            (LinkErrorCode::NotHello, "not_hello"),
            (LinkErrorCode::TooManyIntents, "too_many_intents"),
            (LinkErrorCode::TooManyWaits, "too_many_waits"),
            (LinkErrorCode::StoreFailed, "store_failed"),
            (LinkErrorCode::NoListener, "no_listener"),
            (LinkErrorCode::WrongCode, "wrong_code"),
            (LinkErrorCode::JoinLocked, "join_locked"),
            (LinkErrorCode::NameTaken, "name_taken"),
            (LinkErrorCode::NotLoggedIn, "not_logged_in"),
            (LinkErrorCode::Removed, "removed"),
            (LinkErrorCode::NotLive, "not_live"),
            (LinkErrorCode::LiveLink, "live_link"),
            (LinkErrorCode::LiveReplaced, "live_replaced"),
            (LinkErrorCode::LiveLimit, "live_limit"),
        ] {
            assert_eq!(code.as_str(), name);
            assert_eq!(
                serde_json::to_string(&code).expect("a code is JSON"),
                format!("\"{name}\""),
                "the wire uses the same name as the log"
            );
        }
        assert_eq!(DeviceLogin::Ssh.code(), "ssh");
        assert_eq!(DeviceLogin::Token.code(), "token");
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
        assert_eq!(
            serde_json::to_string(&Frame::Invite(Invite {})).expect("a frame is JSON"),
            r#"{"invite":{}}"#,
            "a request without fields is one empty document"
        );
        assert_eq!(
            serde_json::to_string(&Frame::Devices(Devices {})).expect("a frame is JSON"),
            r#"{"devices":{}}"#
        );
        assert!(
            serde_json::from_str::<Frame>(r#"{"wait":{"since":1},"pull":{"since":1}}"#).is_err(),
            "a frame holds one message"
        );
    }
}
