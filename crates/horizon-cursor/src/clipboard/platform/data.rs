use x11rb::protocol::xproto::{Atom, GetPropertyReply};

use crate::InjectError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ClientIdentity {
    pub(super) resource_client: u32,
    pub(super) pid: Option<u32>,
}

impl ClientIdentity {
    pub(super) fn matches(self, other: Self) -> bool {
        (self.resource_client != 0 && self.resource_client == other.resource_client)
            || self.pid.zip(other.pid).is_some_and(|(left, right)| left == right)
    }
}

pub(super) struct StagedText {
    pub(super) utf8: Vec<u8>,
    pub(super) latin1: Option<Vec<u8>>,
}

impl StagedText {
    pub(super) fn new(text: &str) -> Self {
        Self {
            utf8: text.as_bytes().to_vec(),
            latin1: text
                .chars()
                .map(|character| u8::try_from(u32::from(character)).ok())
                .collect(),
        }
    }
}

#[derive(Default)]
pub(super) struct ClipboardSnapshot {
    pub(super) targets: Vec<StoredTarget>,
    pub(super) total_bytes: usize,
}

pub(super) struct StoredTarget {
    pub(super) target: Atom,
    pub(super) property_type: Atom,
    pub(super) value: StoredValue,
}

impl StoredTarget {
    pub(super) fn from_reply(target: Atom, reply: &GetPropertyReply) -> Result<Self, InjectError> {
        Ok(Self {
            target,
            property_type: reply.type_,
            value: StoredValue::from_reply(reply)?,
        })
    }
}

pub(super) enum StoredValue {
    Bytes8(Vec<u8>),
    Bytes16(Vec<u16>),
    Bytes32(Vec<u32>),
}

impl StoredValue {
    pub(super) fn from_reply(reply: &GetPropertyReply) -> Result<Self, InjectError> {
        match reply.format {
            8 => Ok(Self::Bytes8(reply.value.clone())),
            16 => {
                let values = reply
                    .value16()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid data"))?;
                Ok(Self::Bytes16(values.collect()))
            }
            32 => {
                let values = reply
                    .value32()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid data"))?;
                Ok(Self::Bytes32(values.collect()))
            }
            _ => Err(InjectError::Clipboard("clipboard returned an unsupported data format")),
        }
    }

    pub(super) fn extend_reply(&mut self, reply: &GetPropertyReply) -> Result<(), InjectError> {
        if self.byte_len() == 0 {
            *self = Self::from_reply(reply)?;
            return Ok(());
        }
        match (self, reply.format) {
            (Self::Bytes8(value), 8) => value.extend_from_slice(&reply.value),
            (Self::Bytes16(value), 16) => value.extend(
                reply
                    .value16()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid data"))?,
            ),
            (Self::Bytes32(value), 32) => value.extend(
                reply
                    .value32()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid data"))?,
            ),
            _ => return Err(InjectError::Clipboard("clipboard changed format while being read")),
        }
        Ok(())
    }

    pub(super) fn byte_len(&self) -> usize {
        match self {
            Self::Bytes8(value) => value.len(),
            Self::Bytes16(value) => value.len().saturating_mul(2),
            Self::Bytes32(value) => value.len().saturating_mul(4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientIdentity, StagedText};

    const fn identity(resource_client: u32, pid: u32) -> ClientIdentity {
        ClientIdentity {
            resource_client,
            pid: Some(pid),
        }
    }

    #[test]
    fn client_identity_matches_same_x_client_or_process() {
        let target = identity(0x20_0000, 41);
        assert!(target.matches(identity(0x20_0000, 99)));
        assert!(target.matches(identity(0x30_0000, 41)));
        assert!(!target.matches(identity(0x30_0000, 99)));
    }

    #[test]
    fn staged_text_offers_latin1_only_when_lossless() {
        let expected = [b'b', b'l', 0xe5];
        assert_eq!(StagedText::new("blå").latin1.as_deref(), Some(expected.as_slice()));
        assert!(StagedText::new("hello 🙂").latin1.is_none());
    }
}
