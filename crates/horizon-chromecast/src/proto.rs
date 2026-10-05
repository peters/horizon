//! Cast v2 `CastMessage` protobuf codec and the 32-bit length-prefixed framing.
use crate::{Error, Result};

/// Receivers reject frames above 64 KiB.
pub(crate) const MAX_FRAME: usize = 64 * 1024;

const FIELD_PROTOCOL_VERSION: u64 = 1;
const FIELD_SOURCE_ID: u64 = 2;
const FIELD_DESTINATION_ID: u64 = 3;
const FIELD_NAMESPACE: u64 = 4;
const FIELD_PAYLOAD_TYPE: u64 = 5;
const FIELD_PAYLOAD_UTF8: u64 = 6;
const FIELD_PAYLOAD_BINARY: u64 = 7;

const WIRE_VARINT: u64 = 0;
const WIRE_FIXED64: u64 = 1;
const WIRE_LEN: u64 = 2;
const WIRE_FIXED32: u64 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Payload {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CastMessage {
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) namespace: String,
    pub(crate) payload: Payload,
}

impl CastMessage {
    pub(crate) fn text(source: &str, destination: &str, namespace: &str, payload: String) -> Self {
        Self {
            source: source.to_owned(),
            destination: destination.to_owned(),
            namespace: namespace.to_owned(),
            payload: Payload::Text(payload),
        }
    }

    /// Encodes the message with its 4-byte big-endian length prefix.
    pub(crate) fn encode_frame(&self) -> Result<Vec<u8>> {
        let mut body = Vec::with_capacity(64 + self.namespace.len());
        put_varint_field(&mut body, FIELD_PROTOCOL_VERSION, 0);
        put_bytes_field(&mut body, FIELD_SOURCE_ID, self.source.as_bytes());
        put_bytes_field(&mut body, FIELD_DESTINATION_ID, self.destination.as_bytes());
        put_bytes_field(&mut body, FIELD_NAMESPACE, self.namespace.as_bytes());
        match &self.payload {
            Payload::Text(text) => {
                put_varint_field(&mut body, FIELD_PAYLOAD_TYPE, 0);
                put_bytes_field(&mut body, FIELD_PAYLOAD_UTF8, text.as_bytes());
            }
            Payload::Binary(bytes) => {
                put_varint_field(&mut body, FIELD_PAYLOAD_TYPE, 1);
                put_bytes_field(&mut body, FIELD_PAYLOAD_BINARY, bytes);
            }
        }
        if body.len() > MAX_FRAME {
            return Err(Error::Protocol("message exceeds 64 KiB"));
        }
        let length = u32::try_from(body.len()).map_err(|_| Error::Protocol("message length"))?;
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&body);
        Ok(frame)
    }

    pub(crate) fn decode(mut body: &[u8]) -> Result<Self> {
        let mut message = Self {
            source: String::new(),
            destination: String::new(),
            namespace: String::new(),
            payload: Payload::Binary(Vec::new()),
        };
        let mut text = None;
        let mut binary = None;
        while !body.is_empty() {
            let key = take_varint(&mut body)?;
            let (field, wire) = (key >> 3, key & 7);
            match wire {
                WIRE_VARINT => {
                    take_varint(&mut body)?;
                }
                WIRE_LEN => {
                    let length =
                        usize::try_from(take_varint(&mut body)?).map_err(|_| Error::Protocol("field length"))?;
                    let bytes = take(&mut body, length)?;
                    match field {
                        FIELD_SOURCE_ID => message.source = utf8(bytes)?,
                        FIELD_DESTINATION_ID => message.destination = utf8(bytes)?,
                        FIELD_NAMESPACE => message.namespace = utf8(bytes)?,
                        FIELD_PAYLOAD_UTF8 => text = Some(utf8(bytes)?),
                        FIELD_PAYLOAD_BINARY => binary = Some(bytes.to_vec()),
                        _ => {}
                    }
                }
                WIRE_FIXED64 => {
                    take(&mut body, 8)?;
                }
                WIRE_FIXED32 => {
                    take(&mut body, 4)?;
                }
                _ => return Err(Error::Protocol("unsupported protobuf wire type")),
            }
        }
        message.payload = match (text, binary) {
            (Some(text), _) => Payload::Text(text),
            (None, Some(binary)) => Payload::Binary(binary),
            (None, None) => return Err(Error::Protocol("message without payload")),
        };
        Ok(message)
    }
}

/// Splits complete frames off the front of `buffer`, leaving any partial frame in place.
pub(crate) fn drain_frames(buffer: &mut Vec<u8>) -> Result<Vec<CastMessage>> {
    let mut messages = Vec::new();
    let mut offset = 0;
    while let Some(header) = buffer.get(offset..offset + 4) {
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if length > MAX_FRAME {
            return Err(Error::Protocol("frame exceeds 64 KiB"));
        }
        let Some(body) = buffer.get(offset + 4..offset + 4 + length) else {
            break;
        };
        messages.push(CastMessage::decode(body)?);
        offset += 4 + length;
    }
    buffer.drain(..offset);
    Ok(messages)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value.to_le_bytes()[0] | 0x80);
        value >>= 7;
    }
    out.push(value.to_le_bytes()[0]);
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_varint(out, field << 3 | WIRE_VARINT);
    put_varint(out, value);
}

fn put_bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, field << 3 | WIRE_LEN);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn take_varint(input: &mut &[u8]) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input.split_first().ok_or(Error::Protocol("truncated varint"))?;
        *input = rest;
        // The tenth byte carries only bit 63; anything larger would be truncated.
        if shift == 63 && byte > 1 {
            return Err(Error::Protocol("varint overflows 64 bits"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::Protocol("varint too long"))
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    if input.len() < length {
        return Err(Error::Protocol("truncated field"));
    }
    let (head, rest) = input.split_at(length);
    *input = rest;
    Ok(head)
}

fn utf8(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::Protocol("invalid UTF-8 string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heartbeat() -> CastMessage {
        CastMessage::text(
            "sender-0",
            "receiver-0",
            "urn:x-cast:com.google.cast.tp.heartbeat",
            r#"{"type":"PING"}"#.to_owned(),
        )
    }

    #[test]
    fn encodes_known_bytes() {
        let frame = heartbeat().encode_frame().unwrap();
        let body = &frame[4..];
        assert_eq!(u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize, body.len());
        assert_eq!(&body[..4], &[0x08, 0x00, 0x12, 0x08]);
        assert_eq!(&body[4..12], b"sender-0");
        assert!(body.ends_with(br#"{"type":"PING"}"#));
    }

    #[test]
    fn round_trips_text_and_binary() {
        let text = heartbeat();
        let mut binary = heartbeat();
        binary.payload = Payload::Binary(vec![0, 1, 2, 255]);
        for message in [text, binary] {
            let frame = message.encode_frame().unwrap();
            assert_eq!(CastMessage::decode(&frame[4..]).unwrap(), message);
        }
    }

    #[test]
    fn drains_only_complete_frames() {
        let frame = heartbeat().encode_frame().unwrap();
        let mut buffer = frame.clone();
        buffer.extend_from_slice(&frame[..7]);
        let messages = drain_frames(&mut buffer).unwrap();
        assert_eq!(messages, vec![heartbeat()]);
        assert_eq!(buffer, frame[..7]);
        buffer.extend_from_slice(&frame[7..]);
        assert_eq!(drain_frames(&mut buffer).unwrap().len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn skips_unknown_fields_and_rejects_garbage() {
        let mut body = heartbeat().encode_frame().unwrap()[4..].to_vec();
        body.extend_from_slice(&[0x45, 1, 2, 3, 4, 0x49, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(CastMessage::decode(&body).unwrap(), heartbeat());
        assert!(CastMessage::decode(&[0x12, 0x10, b'a']).is_err());
        assert!(CastMessage::decode(&[0x0b]).is_err());
        let mut overflow = vec![0x08];
        overflow.extend_from_slice(&[0x80; 9]);
        overflow.push(0x02);
        assert!(CastMessage::decode(&overflow).is_err());
        let mut oversized = vec![0xff, 0xff, 0xff, 0xff];
        assert!(drain_frames(&mut oversized).is_err());
    }
}
