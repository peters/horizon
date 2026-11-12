//! The control messages of a frame source program: the framing of one message,
//! the messages that a program sends, and the messages that Horizon sends.
//!
//! A message is 4 bytes of its length, most significant byte first, and then one
//! JSON object of at most [`MAX_BODY_BYTES`] bytes with a member `type`. A type and
//! a member that Horizon does not know are ignored, so a newer program and a newer
//! Horizon can talk: an unknown type is no message, and an unknown member of a
//! known message is no member. A body that is not JSON, a length above the bound
//! and a known message without its own members are one typed failure, because the
//! stream cannot be read any further.
use std::io::{self, Read, Write};

use serde::Deserialize;

use crate::Error;

/// The largest body of one control message.
pub const MAX_BODY_BYTES: usize = 65_536;

/// The bytes of the length of one message.
pub const LENGTH_BYTES: usize = 4;

/// The version of the contract that this Horizon speaks.
pub const PROTOCOL: u32 = 1;

/// The one format of a picture that this Horizon reads.
pub const FORMAT: &str = "nv12";

/// What the program said about itself, once, at the start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// The name of the program, as it says it.
    pub source: String,
    /// The name of the session that the program opened.
    pub session: String,
    /// Whether the program reads `input` messages.
    pub input: bool,
    /// Why the program reads no input, when it says why.
    pub input_reason: Option<String>,
}

/// The size of the stream and of the source that feeds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSize {
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
}

/// The phase of the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Live,
    Hidden,
    Ended,
}

/// What the program says about itself now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub phase: Phase,
    pub reason: Option<String>,
}

/// The counters that the program sends each second, also while hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// The time from the capture of the newest picture that the program sent to the
    /// moment of this message, in milliseconds. A hidden viewer receives no picture,
    /// so this number grows while it is hidden, and it is no clock of Horizon.
    pub age_ms: u64,
}

/// One message of the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Stream(StreamSize),
    State(State),
    Stats(Stats),
    Ok {
        id: Option<String>,
    },
    /// The program refused one message of Horizon. It never ends the viewer.
    Refused {
        code: String,
        message: String,
        id: Option<String>,
    },
    /// A type that this Horizon does not know. It is no message, and no error.
    Ignored,
}

/// One message that Horizon sends to the program.
#[derive(Clone, Debug, PartialEq)]
pub enum Outgoing {
    /// The size to send, whether the viewer is drawn, and an optional id.
    View {
        width: Option<u32>,
        height: Option<u32>,
        visible: Option<bool>,
        id: Option<String>,
    },
    /// The viewer ended: the program exits with 0.
    Bye,
    /// One input event, as the `input` message of the contract.
    Input(crate::input::InputEvent),
}

/// The length bytes of one body, most significant byte first.
#[must_use]
pub fn length_bytes(body: usize) -> [u8; LENGTH_BYTES] {
    u32::try_from(body).unwrap_or(u32::MAX).to_be_bytes()
}

/// The next message of `reader`, or `None` at the end of the stream.
///
/// # Errors
/// Reports a length above [`MAX_BODY_BYTES`], a body that ended early, a body that
/// is not JSON and a known message without its own members.
pub fn read(reader: &mut impl Read) -> Result<Option<Message>, Error> {
    let mut length = [0_u8; LENGTH_BYTES];
    match read_exact_or_end(reader, &mut length)? {
        0 => return Ok(None),
        read if read < LENGTH_BYTES => {
            return Err(Error::Control(format!(
                "the length of a message ended after {read} bytes of {LENGTH_BYTES}"
            )));
        }
        _ => {}
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_BODY_BYTES {
        return Err(Error::Control(format!(
            "a message of {length} bytes is larger than the {MAX_BODY_BYTES} bytes of one message"
        )));
    }
    let mut body = vec![0_u8; length];
    let read = read_exact_or_end(reader, &mut body)?;
    if read < length {
        return Err(Error::Control(format!(
            "a message of {length} bytes ended after {read} bytes"
        )));
    }
    decode(&body).map(Some)
}

/// The message of one body.
///
/// # Errors
/// Reports a body that is not one JSON object with a member `type`, and a known
/// message without its own members.
pub fn decode(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
    }
    let envelope: Envelope = parse_body(body)?;
    match envelope.kind.as_str() {
        "hello" => hello(body),
        "stream" => stream(body),
        "state" => state(body),
        "stats" => stats(body),
        "ok" => ok(body),
        "error" => refused(body),
        _ => Ok(Message::Ignored),
    }
}

fn hello(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        protocol: u32,
        formats: Vec<String>,
        source: String,
        session: String,
        #[serde(default)]
        input: Option<bool>,
        #[serde(default)]
        input_reason: Option<String>,
    }
    let wire: Wire = parse_body(body)?;
    if wire.protocol != PROTOCOL {
        return Err(Error::Unsupported(format!(
            "speaks protocol {}, and this Horizon reads {PROTOCOL}",
            wire.protocol
        )));
    }
    if !wire.formats.iter().any(|format| format == FORMAT) {
        return Err(Error::Unsupported(format!(
            "sends no {FORMAT} picture, and {FORMAT} is the only format that this Horizon reads"
        )));
    }
    Ok(Message::Hello(Hello {
        source: wire.source,
        session: wire.session,
        input: wire.input.unwrap_or(false),
        input_reason: wire.input_reason,
    }))
}

fn stream(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        width: u32,
        height: u32,
        source_width: u32,
        source_height: u32,
    }
    let wire: Wire = parse_body(body)?;
    Ok(Message::Stream(StreamSize {
        width: wire.width,
        height: wire.height,
        source_width: wire.source_width,
        source_height: wire.source_height,
    }))
}

fn state(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        phase: String,
        #[serde(default)]
        reason: Option<String>,
    }
    let wire: Wire = parse_body(body)?;
    let phase = match wire.phase.as_str() {
        "live" => Phase::Live,
        "hidden" => Phase::Hidden,
        "ended" => Phase::Ended,
        // A phase that this Horizon does not know says nothing about the stream,
        // so it is no message.
        _ => return Ok(Message::Ignored),
    };
    Ok(Message::State(State {
        phase,
        reason: wire.reason,
    }))
}

fn stats(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        frames_sent: u64,
        frames_dropped: u64,
        bytes: u64,
        width: u32,
        height: u32,
        age_ms: u64,
    }
    let wire: Wire = parse_body(body)?;
    Ok(Message::Stats(Stats {
        frames_sent: wire.frames_sent,
        frames_dropped: wire.frames_dropped,
        bytes: wire.bytes,
        width: wire.width,
        height: wire.height,
        age_ms: wire.age_ms,
    }))
}

fn ok(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        id: Option<String>,
    }
    let wire: Wire = parse_body(body)?;
    Ok(Message::Ok { id: wire.id })
}

fn refused(body: &[u8]) -> Result<Message, Error> {
    #[derive(Deserialize)]
    struct Wire {
        code: String,
        message: String,
        #[serde(default)]
        id: Option<String>,
    }
    let wire: Wire = parse_body(body)?;
    Ok(Message::Refused {
        code: wire.code,
        message: wire.message,
        id: wire.id,
    })
}

impl Outgoing {
    /// The body of this message, as the JSON object of the contract.
    ///
    /// # Errors
    /// Reports a message that cannot be encoded, which no message of this type
    /// causes.
    pub fn body(&self) -> Result<Vec<u8>, Error> {
        let value = match self {
            Self::View {
                width,
                height,
                visible,
                id,
            } => {
                let mut object = serde_json::Map::new();
                object.insert("type".into(), "view".into());
                // Each member is optional, and a member that this message does not
                // carry is absent rather than null.
                if let Some(width) = width {
                    object.insert("width".into(), i64::from(*width).into());
                }
                if let Some(height) = height {
                    object.insert("height".into(), i64::from(*height).into());
                }
                if let Some(visible) = visible {
                    object.insert("visible".into(), (*visible).into());
                }
                if let Some(id) = id {
                    object.insert("id".into(), id.as_str().into());
                }
                serde_json::Value::Object(object)
            }
            Self::Bye => serde_json::json!({ "type": "bye" }),
            Self::Input(event) => event.body(),
        };
        serde_json::to_vec(&value).map_err(|error| Error::Control(format!("a message is not JSON: {error}")))
    }

    /// The bytes of this message: its length and then its body.
    ///
    /// # Errors
    /// Reports a body above [`MAX_BODY_BYTES`] and a message that cannot be
    /// encoded.
    pub fn frame(&self) -> Result<Vec<u8>, Error> {
        let body = self.body()?;
        if body.len() > MAX_BODY_BYTES {
            return Err(Error::Control(format!(
                "one message of {MAX_BODY_BYTES} bytes is the largest that Horizon sends"
            )));
        }
        let mut bytes = Vec::with_capacity(LENGTH_BYTES + body.len());
        bytes.extend_from_slice(&length_bytes(body.len()));
        bytes.extend_from_slice(&body);
        Ok(bytes)
    }
}

/// Write one message and flush it, so the program reads it now.
///
/// # Errors
/// Reports a message that cannot be encoded and a stream that a program closed.
pub fn write(stream: &mut impl Write, message: &Outgoing) -> Result<(), Error> {
    stream.write_all(&message.frame()?)?;
    stream.flush()?;
    Ok(())
}

/// One body as the struct of its own message.
fn parse_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(body)
        .map_err(|error| Error::Control(format!("a message is not one JSON object of its type: {error}")))
}

fn read_exact_or_end(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, Error> {
    let mut read = 0;
    while read < buffer.len() {
        match reader.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(bytes) => read += bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::from(error)),
        }
    }
    Ok(read)
}

/// One `view` message for an optional size and an optional visibility.
#[must_use]
pub fn view(width: Option<u32>, height: Option<u32>, visible: Option<bool>) -> Outgoing {
    Outgoing::View {
        width,
        height,
        visible,
        id: None,
    }
}

/// One `hello` body, for a test and for a stand-in that opens a session.
#[must_use]
pub fn hello_body(hello: &Hello) -> serde_json::Value {
    let mut object = serde_json::json!({
        "type": "hello",
        "protocol": PROTOCOL,
        "formats": [FORMAT],
        "source": hello.source,
        "session": hello.session,
        "input": hello.input,
    });
    if let Some(reason) = &hello.input_reason {
        object["input_reason"] = reason.as_str().into();
    }
    object
}

/// One body of the `stats` message, for a test and for a stand-in.
#[must_use]
pub fn stats_body(stats: &Stats) -> serde_json::Value {
    serde_json::json!({
        "type": "stats",
        "frames_sent": stats.frames_sent,
        "frames_dropped": stats.frames_dropped,
        "bytes": stats.bytes,
        "width": stats.width,
        "height": stats.height,
        "age_ms": stats.age_ms,
    })
}

/// The body of one `state` message, for a test and for a stand-in.
#[must_use]
pub fn state_body(state: &State) -> serde_json::Value {
    let phase = match state.phase {
        Phase::Live => "live",
        Phase::Hidden => "hidden",
        Phase::Ended => "ended",
    };
    let mut object = serde_json::json!({ "type": "state", "phase": phase });
    if let Some(reason) = &state.reason {
        object["reason"] = reason.as_str().into();
    }
    object
}

/// The body of one `stream` message, for a test and for a stand-in.
#[must_use]
pub fn stream_body(size: &StreamSize) -> serde_json::Value {
    serde_json::json!({
        "type": "stream",
        "width": size.width,
        "height": size.height,
        "source_width": size.source_width,
        "source_height": size.source_height,
    })
}

/// The message of one body, for a test and for a stand-in that reads a `view`.
#[must_use]
pub fn decode_json(value: &serde_json::Value) -> Option<Message> {
    let body = serde_json::to_vec(value).ok()?;
    decode(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(value: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(value).expect("a body");
        let mut bytes = length_bytes(body.len()).to_vec();
        bytes.extend_from_slice(&body);
        bytes
    }

    #[test]
    fn the_length_of_a_message_comes_most_significant_byte_first() {
        assert_eq!(length_bytes(0), [0, 0, 0, 0]);
        assert_eq!(length_bytes(1), [0, 0, 0, 1]);
        assert_eq!(length_bytes(258), [0, 0, 1, 2]);
        assert_eq!(length_bytes(MAX_BODY_BYTES), [0, 1, 0, 0]);

        let mut stream = Cursor::new(framed(&serde_json::json!({"type": "ok", "id": "a"})));
        let message = read(&mut stream).expect("one message").expect("a message");
        assert_eq!(message, Message::Ok { id: Some("a".into()) });
        assert!(read(&mut stream).expect("the end").is_none());
    }

    #[test]
    fn a_message_above_the_bound_and_text_that_is_not_json_are_refused() {
        // The length alone is enough: no body is read for a message that is too
        // large.
        let mut stream = Cursor::new(vec![0, 1, 0, 1]);
        let error = read(&mut stream).expect_err("a message above the bound");
        assert!(error.to_string().contains("65536"), "{error}");

        let mut stream = Cursor::new(vec![0, 0, 0, 5, b'h', b'e', b'l', b'l', b'o']);
        let error = read(&mut stream).expect_err("text that is not JSON");
        assert!(matches!(error, Error::Control(_)), "{error:?}");

        // A body of exactly the bound is read, and a JSON body that holds an
        // unknown type is no message.
        let mut object = serde_json::json!({"type": "something-new"});
        object["padding"] = "x".repeat(MAX_BODY_BYTES - 40).into();
        let body = serde_json::to_vec(&object).expect("a body");
        assert!(body.len() <= MAX_BODY_BYTES);
        let mut stream = Cursor::new(framed(&object));
        assert_eq!(read(&mut stream).expect("one message"), Some(Message::Ignored));
    }

    #[test]
    fn a_message_that_ended_early_is_one_failure() {
        let mut stream = Cursor::new(vec![0, 0, 0, 40, b'{']);
        let error = read(&mut stream).expect_err("a message that ended early");
        assert!(error.to_string().contains("ended after"), "{error}");
        let mut stream = Cursor::new(vec![0, 0]);
        read(&mut stream).expect_err("a length that ended early");
    }

    #[test]
    fn an_unknown_type_and_an_unknown_member_are_ignored() {
        assert_eq!(decode(br#"{"type": "another-message", "x": 1}"#), Ok(Message::Ignored));
        assert_eq!(
            decode(br#"{"type": "stats", "frames_sent": 3, "frames_dropped": 1, "bytes": 9, "width": 640, "height": 480, "age_ms": 20, "extra": [1, 2]}"#),
            Ok(Message::Stats(Stats {
                frames_sent: 3,
                frames_dropped: 1,
                bytes: 9,
                width: 640,
                height: 480,
                age_ms: 20,
            }))
        );
        assert_eq!(decode(br#"{"type": "state", "phase": "paused"}"#), Ok(Message::Ignored));
        // A known message without its own members is one typed failure.
        let error = decode(br#"{"type": "stats", "frames_sent": 3}"#).expect_err("an incomplete message");
        assert!(matches!(error, Error::Control(_)));
        decode(br#"{"no-type": 1}"#).expect_err("a message without a type");
    }

    #[test]
    fn the_messages_of_a_program_are_read_as_the_contract_names_them() {
        assert_eq!(
            decode_json(&hello_body(&Hello {
                source: "a stand-in".into(),
                session: "s-1".into(),
                input: true,
                input_reason: None,
            })),
            Some(Message::Hello(Hello {
                source: "a stand-in".into(),
                session: "s-1".into(),
                input: true,
                input_reason: None,
            }))
        );
        let ended = State {
            phase: Phase::Ended,
            reason: Some("the source said stop".into()),
        };
        assert_eq!(decode_json(&state_body(&ended)), Some(Message::State(ended)));
        assert_eq!(
            decode_json(&stream_body(&StreamSize {
                width: 1280,
                height: 720,
                source_width: 1920,
                source_height: 1080,
            })),
            Some(Message::Stream(StreamSize {
                width: 1280,
                height: 720,
                source_width: 1920,
                source_height: 1080,
            }))
        );
        // A hello of another protocol version or of another format ends the
        // viewer: this Horizon cannot read those pictures.
        for hello in [
            serde_json::json!({"type": "hello", "protocol": 2, "formats": ["nv12"], "source": "s", "session": "x"}),
            serde_json::json!({"type": "hello", "protocol": 1, "formats": ["yuv444"], "source": "s", "session": "x"}),
        ] {
            let body = serde_json::to_vec(&hello).expect("a body");
            let error = decode(&body).expect_err("a protocol that this Horizon does not speak");
            assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
        }
    }

    #[test]
    fn the_messages_of_horizon_carry_only_the_members_that_they_set() {
        let mut stream = Vec::new();
        write(&mut stream, &view(None, None, Some(false))).expect("one view message");
        let body = &stream[LENGTH_BYTES..];
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).expect("a body"),
            serde_json::json!({"type": "view", "visible": false})
        );

        let mut stream = Vec::new();
        write(&mut stream, &view(Some(640), Some(480), Some(true))).expect("one view message");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&stream[LENGTH_BYTES..]).expect("a body"),
            serde_json::json!({"type": "view", "width": 640, "height": 480, "visible": true})
        );

        let mut stream = Vec::new();
        write(&mut stream, &Outgoing::Bye).expect("one bye message");
        assert_eq!(
            &stream[..],
            &[
                0, 0, 0, 14, b'{', b'"', b't', b'y', b'p', b'e', b'"', b':', b'"', b'b', b'y', b'e', b'"', b'}'
            ]
        );
    }
}
