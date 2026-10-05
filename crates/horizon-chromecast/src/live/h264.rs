//! H.264 access-unit helpers: AVCC to Annex B and access unit delimiters.
use crate::{Error, Result};

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const ACCESS_UNIT_DELIMITER: [u8; 6] = [0, 0, 0, 1, 0x09, 0xf0];
const NAL_IDR: u8 = 5;
const NAL_AUD: u8 = 9;

/// Converts a length-prefixed (AVCC) sample to Annex B. Parameter sets are
/// prepended in front of IDR pictures so every segment starts decodable.
/// # Errors
/// Returns a protocol error for an invalid NAL length size or truncated NAL.
pub fn avcc_to_annexb(sample: &[u8], length_size: usize, parameter_sets: &[&[u8]]) -> Result<Vec<u8>> {
    if !(1..=4).contains(&length_size) {
        return Err(Error::Protocol("NAL length size must be 1 to 4 bytes"));
    }
    let mut nals = Vec::new();
    let mut rest = sample;
    while !rest.is_empty() {
        let (prefix, tail) = rest
            .split_at_checked(length_size)
            .ok_or(Error::Protocol("truncated NAL length"))?;
        let length = prefix.iter().fold(0usize, |acc, &b| acc << 8 | usize::from(b));
        let (nal, tail) = tail
            .split_at_checked(length)
            .ok_or(Error::Protocol("truncated NAL unit"))?;
        nals.push(nal);
        rest = tail;
    }
    let idr = nals.iter().any(|nal| nal_type(nal) == Some(NAL_IDR));
    let mut out = Vec::with_capacity(sample.len() + 64);
    let parameter_sets = if idr { parameter_sets } else { &[] };
    for nal in parameter_sets.iter().copied().chain(nals) {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
    }
    Ok(out)
}

/// Prefixes an access unit delimiter unless the access unit already has one.
pub(crate) fn with_delimiter(annexb: &[u8]) -> Vec<u8> {
    let first = first_nal(annexb);
    let mut out = Vec::with_capacity(annexb.len() + ACCESS_UNIT_DELIMITER.len());
    if first.and_then(nal_type) != Some(NAL_AUD) {
        out.extend_from_slice(&ACCESS_UNIT_DELIMITER);
    }
    out.extend_from_slice(annexb);
    out
}

fn first_nal(annexb: &[u8]) -> Option<&[u8]> {
    let start = annexb.windows(3).position(|w| w == [0, 0, 1])? + 3;
    annexb.get(start..)
}

fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|header| header & 0x1f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_avcc_and_adds_parameter_sets_to_idr() {
        let sps: &[u8] = &[0x67, 1, 2];
        let pps: &[u8] = &[0x68, 3];
        let idr = [0, 0, 0, 2, 0x65, 9];
        let annexb = avcc_to_annexb(&idr, 4, &[sps, pps]).unwrap();
        assert_eq!(
            annexb,
            [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9]
        );
        let slice = [0, 2, 0x41, 7];
        assert_eq!(avcc_to_annexb(&slice, 2, &[sps]).unwrap(), [0, 0, 0, 1, 0x41, 7]);
        assert!(avcc_to_annexb(&[0, 0, 0, 9, 1], 4, &[]).is_err());
        assert!(avcc_to_annexb(&idr, 0, &[]).is_err());
    }

    #[test]
    fn delimiter_is_added_once() {
        let unit = [0, 0, 0, 1, 0x65, 1];
        let delimited = with_delimiter(&unit);
        assert_eq!(&delimited[..6], &ACCESS_UNIT_DELIMITER);
        assert_eq!(with_delimiter(&delimited), delimited);
    }
}
