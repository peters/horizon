use crate::{Error, Result};
use std::collections::BTreeMap;

pub(crate) fn encode(fields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut output = Vec::new();
    for &(tag, data) in fields {
        if data.is_empty() {
            output.extend([tag, 0]);
        }
        for part in data.chunks(255) {
            output.extend([tag, u8::try_from(part.len()).unwrap_or(255)]);
            output.extend(part);
        }
    }
    output
}

pub(crate) struct Message(BTreeMap<u8, Vec<u8>>);
impl Message {
    pub(crate) fn parse(mut data: &[u8]) -> Result<Self> {
        if data.len() > 8192 {
            return Err(Error::Protocol("pairing body too large"));
        }
        let mut fields: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
        let mut previous = None;
        while !data.is_empty() {
            let [tag, length, ..] = data else {
                return Err(Error::Protocol("truncated TLV header"));
            };
            let tag = *tag;
            let length = usize::from(*length);
            let value = data.get(2..2 + length).ok_or(Error::Protocol("truncated TLV value"))?;
            if fields.contains_key(&tag) && previous != Some((tag, 255)) {
                return Err(Error::Protocol("duplicate TLV field"));
            }
            fields.entry(tag).or_default().extend(value);
            previous = Some((tag, length));
            data = &data[2 + length..];
        }
        Ok(Self(fields))
    }
    pub(crate) fn get(&self, tag: u8) -> Result<&[u8]> {
        self.0
            .get(&tag)
            .map(Vec::as_slice)
            .ok_or(Error::Protocol("missing TLV field"))
    }
    pub(crate) fn state(&self, expected: u8, phase: &'static str) -> Result<()> {
        if let Some(error) = self.0.get(&7) {
            return Err(Error::Pairing {
                phase,
                step: expected,
                code: *error.first().ok_or(Error::Protocol("empty pairing error"))?,
            });
        }
        if self.get(6)? != [expected] {
            return Err(Error::Protocol("unexpected pairing state"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragments_and_rejects_ambiguous_messages() {
        let value = vec![42; 768];
        let encoded = encode(&[(3, &value), (6, &[2])]);
        let decoded = Message::parse(&encoded).expect("valid message");
        assert_eq!(decoded.get(3).expect("key"), value);
        assert!(decoded.state(2, "pair-setup").is_ok());
        assert!(decoded.state(4, "pair-setup").is_err());
        for invalid in [&[3][..], &[3, 3, 1], &[6, 1, 2, 6, 1, 4]] {
            assert!(Message::parse(invalid).is_err());
        }
        let rejected = Message::parse(&[6, 1, 2, 7, 1, 2]).expect("error message");
        assert!(matches!(
            rejected.state(2, "pair-setup"),
            Err(Error::Pairing { code: 2, .. })
        ));
    }
}
