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
        let latin1 = text
            .chars()
            .map(|character| u8::try_from(u32::from(character)).ok())
            .collect();
        Self {
            utf8: text.as_bytes().to_vec(),
            latin1,
        }
    }
}

#[derive(Default)]
pub(super) struct ClipboardSnapshot {
    pub(super) targets: Vec<StoredTarget>,
    pub(super) total_bytes: usize,
}

impl ClipboardSnapshot {
    pub(super) fn find(&self, target: Atom) -> Option<&StoredTarget> {
        self.targets.iter().find(|stored| stored.target == target)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

pub(super) struct StoredTarget {
    pub(super) target: Atom,
    pub(super) property_type: Atom,
    pub(super) format: u8,
    pub(super) value: StoredValue,
}

impl StoredTarget {
    pub(super) fn from_reply(target: Atom, reply: &GetPropertyReply) -> Result<Self, InjectError> {
        let value = StoredValue::from_reply(reply)?;
        Ok(Self {
            target,
            property_type: reply.type_,
            format: reply.format,
            value,
        })
    }
}

pub(super) enum StoredValue {
    Bytes8(Vec<u8>),
    Bytes16(Vec<u16>),
    Bytes32(Vec<u32>),
}

impl Default for StoredValue {
    fn default() -> Self {
        Self::Bytes8(Vec::new())
    }
}

impl StoredValue {
    pub(super) fn from_reply(reply: &GetPropertyReply) -> Result<Self, InjectError> {
        match reply.format {
            8 => Ok(Self::Bytes8(reply.value.clone())),
            16 => Ok(Self::Bytes16(
                reply
                    .value16()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid 16-bit data"))?
                    .collect(),
            )),
            32 => Ok(Self::Bytes32(
                reply
                    .value32()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid 32-bit data"))?
                    .collect(),
            )),
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
                    .ok_or(InjectError::Clipboard("clipboard returned invalid 16-bit data"))?,
            ),
            (Self::Bytes32(value), 32) => value.extend(
                reply
                    .value32()
                    .ok_or(InjectError::Clipboard("clipboard returned invalid 32-bit data"))?,
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

    pub(super) fn as_u32_slice(&self) -> Option<&[u32]> {
        match self {
            Self::Bytes32(value) => Some(value),
            Self::Bytes8(_) | Self::Bytes16(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientIdentity, StagedText, StoredValue};

    #[test]
    fn client_identity_matches_same_x_client_or_process() {
        let target = ClientIdentity {
            resource_client: 0x20_0000,
            pid: Some(41),
        };
        assert!(target.matches(ClientIdentity {
            resource_client: 0x20_0000,
            pid: Some(99),
        }));
        assert!(target.matches(ClientIdentity {
            resource_client: 0x30_0000,
            pid: Some(41),
        }));
        assert!(!target.matches(ClientIdentity {
            resource_client: 0x30_0000,
            pid: Some(99),
        }));
    }

    #[test]
    fn staged_text_offers_latin1_only_when_lossless() {
        assert_eq!(
            StagedText::new("blå").latin1.as_deref(),
            Some([b'b', b'l', 0xe5].as_slice())
        );
        assert!(StagedText::new("hello 🙂").latin1.is_none());
    }

    #[test]
    fn stored_value_reports_encoded_byte_length() {
        assert_eq!(StoredValue::Bytes8(vec![1, 2, 3]).byte_len(), 3);
        assert_eq!(StoredValue::Bytes16(vec![1, 2, 3]).byte_len(), 6);
        assert_eq!(StoredValue::Bytes32(vec![1, 2, 3]).byte_len(), 12);
    }
}
