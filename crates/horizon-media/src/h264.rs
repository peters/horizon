//! H.264 bitstream helpers: NAL unit types, Annex B and AVCC framing, and an
//! incremental Annex B reader that groups NAL units into access units.
use std::fmt;

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SEI: u8 = 6;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;

/// Encoder output beyond this size is treated as corrupt.
pub const DEFAULT_LIMIT: usize = 8 * 1024 * 1024;

const START_CODE: [u8; 4] = [0, 0, 0, 1];
const ACCESS_UNIT_DELIMITER: [u8; 6] = [0, 0, 0, 1, NAL_AUD, 0xf0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum H264Error {
    LengthSize,
    TruncatedLength,
    TruncatedNal,
    NalTooLarge,
    AccessUnitTooLarge,
}

impl H264Error {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LengthSize => "NAL length size must be 1 to 4 bytes",
            Self::TruncatedLength => "truncated NAL length",
            Self::TruncatedNal => "truncated NAL unit",
            Self::NalTooLarge => "encoder NAL exceeds limit",
            Self::AccessUnitTooLarge => "encoder access unit exceeds limit",
        }
    }
}

impl fmt::Display for H264Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for H264Error {}

/// The `nal_unit_type` of a NAL unit without its start code.
#[must_use]
pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|header| header & 0x1f)
}

/// Converts a length-prefixed (AVCC) sample to Annex B. Parameter sets are
/// prepended in front of IDR pictures so every IDR starts decodable.
/// # Errors
/// Returns an error for an invalid NAL length size or a truncated NAL unit.
pub fn avcc_to_annexb(sample: &[u8], length_size: usize, parameter_sets: &[&[u8]]) -> Result<Vec<u8>, H264Error> {
    if !(1..=4).contains(&length_size) {
        return Err(H264Error::LengthSize);
    }
    let mut nals = Vec::new();
    let mut rest = sample;
    while !rest.is_empty() {
        let (prefix, tail) = rest.split_at_checked(length_size).ok_or(H264Error::TruncatedLength)?;
        let length = prefix.iter().fold(0usize, |acc, &b| acc << 8 | usize::from(b));
        let (nal, tail) = tail.split_at_checked(length).ok_or(H264Error::TruncatedNal)?;
        nals.push(nal);
        rest = tail;
    }
    Ok(annexb(&nals, parameter_sets))
}

/// Prefixes an access unit delimiter unless the access unit already has one.
#[must_use]
pub fn with_delimiter(annexb: &[u8]) -> Vec<u8> {
    let first = start_code(annexb, 0).and_then(|(at, length)| annexb.get(at + length..));
    let mut out = Vec::with_capacity(annexb.len() + ACCESS_UNIT_DELIMITER.len());
    if first.and_then(nal_type) != Some(NAL_AUD) {
        out.extend_from_slice(&ACCESS_UNIT_DELIMITER);
    }
    out.extend_from_slice(annexb);
    out
}

/// Byte length of a leading access unit delimiter, including its start code.
/// Parameter sets inserted into a unit belong after it.
#[must_use]
pub fn leading_delimiter_len(annexb: &[u8]) -> usize {
    let Some((at, length)) = start_code(annexb, 0) else {
        return 0;
    };
    if annexb.get(at + length..).and_then(nal_type) != Some(NAL_AUD) {
        return 0;
    }
    start_code(annexb, at + length).map_or(annexb.len(), |(next, _)| next)
}

fn annexb(nals: &[&[u8]], parameter_sets: &[&[u8]]) -> Vec<u8> {
    let idr = nals.iter().any(|nal| nal_type(nal) == Some(NAL_IDR));
    let parameter_sets = if idr { parameter_sets } else { &[] };
    let size: usize = parameter_sets
        .iter()
        .chain(nals)
        .map(|nal| nal.len() + START_CODE.len())
        .sum();
    // A leading delimiter must stay first, or it would split the access unit.
    let delimiters = nals.iter().take_while(|nal| nal_type(nal) == Some(NAL_AUD)).count();
    let (leading, rest) = nals.split_at(delimiters);
    let mut out = Vec::with_capacity(size);
    for nal in leading.iter().chain(parameter_sets).chain(rest) {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(nal);
    }
    out
}

/// Slice and SEI NAL units of one picture, without start codes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessUnit {
    pub nals: Vec<Vec<u8>>,
}

impl AccessUnit {
    #[must_use]
    pub fn is_keyframe(&self) -> bool {
        self.nals.iter().any(|nal| nal_type(nal) == Some(NAL_IDR))
    }

    #[must_use]
    pub fn nal_refs(&self) -> Vec<&[u8]> {
        self.nals.iter().map(Vec::as_slice).collect()
    }

    /// Annex B bytes, with `parameter_sets` in front of an IDR picture.
    #[must_use]
    pub fn to_annexb(&self, parameter_sets: &[&[u8]]) -> Vec<u8> {
        annexb(&self.nal_refs(), parameter_sets)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    /// A PPS together with the most recent SPS.
    ParameterSets {
        sps: Vec<u8>,
        pps: Vec<u8>,
    },
    AccessUnit(AccessUnit),
}

/// Incremental reader for encoder Annex B output with an access unit delimiter
/// before every picture. A NAL unit completes when the next start code
/// arrives; an access unit completes at the next delimiter.
pub struct AnnexBReader {
    pending: Vec<u8>,
    access: Vec<Vec<u8>>,
    access_bytes: usize,
    sps: Vec<u8>,
    limit: usize,
}

impl Default for AnnexBReader {
    fn default() -> Self {
        Self::new(DEFAULT_LIMIT)
    }
}

impl AnnexBReader {
    /// `limit` bounds both a pending NAL unit and an access unit, in bytes.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            pending: Vec::new(),
            access: Vec::new(),
            access_bytes: 0,
            sps: Vec::new(),
            limit,
        }
    }

    /// Feeds encoder output and returns the units it completed, in order.
    /// # Errors
    /// Returns an error when a NAL unit or access unit exceeds the limit.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Unit>, H264Error> {
        self.pending.extend_from_slice(bytes);
        let mut units = Vec::new();
        while let Some(nal) = take_nal(&mut self.pending) {
            self.accept(nal, &mut units)?;
        }
        // Only the payload of the still-incomplete NAL counts against the
        // limit, as for completed NALs, so chunk boundaries cannot change the
        // outcome. Bytes before the first start code belong to no NAL.
        let payload = if let Some((at, length)) = start_code(&self.pending, 0) {
            self.pending.drain(..at);
            self.pending.len() - length
        } else {
            // Keep a possible partial start code.
            let keep = self.pending.len().min(3);
            self.pending.drain(..self.pending.len() - keep);
            0
        };
        if payload > self.limit {
            return Err(H264Error::NalTooLarge);
        }
        Ok(units)
    }

    /// Completes the trailing NAL unit and access unit at the end of a stream.
    /// # Errors
    /// Returns an error when the final access unit exceeds the limit.
    pub fn finish(&mut self) -> Result<Vec<Unit>, H264Error> {
        let mut units = Vec::new();
        if let Some((at, length)) = start_code(&self.pending, 0) {
            let nal = self.pending[at + length..].to_vec();
            self.pending.clear();
            self.accept(nal, &mut units)?;
        }
        self.complete_access(&mut units);
        Ok(units)
    }

    fn accept(&mut self, nal: Vec<u8>, units: &mut Vec<Unit>) -> Result<(), H264Error> {
        if nal.len() > self.limit {
            return Err(H264Error::NalTooLarge);
        }
        match nal_type(&nal).unwrap_or(0) {
            NAL_AUD => self.complete_access(units),
            NAL_SPS => self.sps = nal,
            NAL_PPS => units.push(Unit::ParameterSets {
                sps: self.sps.clone(),
                pps: nal,
            }),
            NAL_SLICE | NAL_IDR | NAL_SEI => {
                self.access_bytes += nal.len();
                self.access.push(nal);
            }
            _ => {}
        }
        if self.access_bytes > self.limit {
            return Err(H264Error::AccessUnitTooLarge);
        }
        Ok(())
    }

    fn complete_access(&mut self, units: &mut Vec<Unit>) {
        if !self.access.is_empty() {
            self.access_bytes = 0;
            units.push(Unit::AccessUnit(AccessUnit {
                nals: std::mem::take(&mut self.access),
            }));
        }
    }
}

fn take_nal(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let first = start_code(buffer, 0)?;
    let next = start_code(buffer, first.0 + first.1)?;
    let nal = buffer[first.0 + first.1..next.0].to_vec();
    buffer.drain(..next.0);
    Some(nal)
}

fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    for at in from..data.len().saturating_sub(2) {
        if data.get(at..at + 4) == Some(&[0, 0, 0, 1]) {
            return Some((at, 4));
        }
        if data.get(at..at + 3) == Some(&[0, 0, 1]) {
            return Some((at, 3));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 1, 2];
    const PPS: &[u8] = &[0x68, 3];

    #[test]
    fn converts_avcc_and_adds_parameter_sets_to_idr() {
        let idr = [0, 0, 0, 2, 0x65, 9];
        let annexb = avcc_to_annexb(&idr, 4, &[SPS, PPS]).unwrap();
        assert_eq!(
            annexb,
            [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9]
        );
        let slice = [0, 2, 0x41, 7];
        assert_eq!(avcc_to_annexb(&slice, 2, &[SPS]).unwrap(), [0, 0, 0, 1, 0x41, 7]);
        assert_eq!(avcc_to_annexb(&[0, 0, 0, 9, 1], 4, &[]), Err(H264Error::TruncatedNal));
        assert_eq!(avcc_to_annexb(&idr, 0, &[]), Err(H264Error::LengthSize));
    }

    #[test]
    fn delimiter_is_added_once() {
        let unit = [0, 0, 0, 1, 0x65, 1];
        let delimited = with_delimiter(&unit);
        assert_eq!(&delimited[..6], &ACCESS_UNIT_DELIMITER);
        assert_eq!(with_delimiter(&delimited), delimited);
    }

    fn stream() -> Vec<u8> {
        let mut data = Vec::new();
        for nal in [
            &[NAL_AUD, 0xf0][..],
            SPS,
            PPS,
            &[0x65, 4, 4],
            &[0x06, 5],
            &[NAL_AUD, 0xf0],
            &[0x41, 6],
            &[NAL_AUD, 0xf0],
        ] {
            data.extend_from_slice(&START_CODE);
            data.extend_from_slice(nal);
        }
        data
    }

    #[test]
    fn reader_groups_access_units_across_chunk_boundaries() {
        let mut reader = AnnexBReader::default();
        let mut units = Vec::new();
        for chunk in stream().chunks(3) {
            units.extend(reader.push(chunk).unwrap());
        }
        units.extend(reader.finish().unwrap());
        assert_eq!(
            units[0],
            Unit::ParameterSets {
                sps: SPS.to_vec(),
                pps: PPS.to_vec()
            }
        );
        let Unit::AccessUnit(idr) = &units[1] else {
            panic!("expected an access unit");
        };
        assert!(idr.is_keyframe());
        assert_eq!(idr.nal_refs(), [&[0x65, 4, 4][..], &[0x06, 5]]);
        assert_eq!(idr.to_annexb(&[SPS, PPS])[..9], [0, 0, 0, 1, 0x67, 1, 2, 0, 0]);
        let Unit::AccessUnit(slice) = &units[2] else {
            panic!("expected an access unit");
        };
        assert!(!slice.is_keyframe());
        assert_eq!(slice.to_annexb(&[SPS, PPS]), [0, 0, 0, 1, 0x41, 6]);
        assert_eq!(units.len(), 3);
    }

    #[test]
    fn finish_flushes_a_stream_without_trailing_delimiter() {
        let mut reader = AnnexBReader::default();
        let mut data = START_CODE.to_vec();
        data.extend_from_slice(&[0x65, 1]);
        assert!(reader.push(&data).unwrap().is_empty());
        let units = reader.finish().unwrap();
        assert_eq!(
            units,
            [Unit::AccessUnit(AccessUnit {
                nals: vec![vec![0x65, 1]]
            })]
        );
    }

    #[test]
    fn reader_enforces_limits() {
        let mut reader = AnnexBReader::new(8);
        assert_eq!(
            reader.push(&[0, 0, 1, 0x41, 0, 0, 0, 0, 0, 0, 0, 9]),
            Err(H264Error::NalTooLarge)
        );
        let mut reader = AnnexBReader::new(10);
        assert!(reader.push(&[0, 0, 1, 0x41, 1, 2, 3, 0, 0, 1]).is_ok());
        assert!(reader.push(&[0x41, 1, 2, 3, 0, 0, 1]).is_ok());
        assert_eq!(reader.push(&[0x41, 1, 2, 0, 0, 1]), Err(H264Error::AccessUnitTooLarge));
    }

    #[test]
    fn parameter_sets_go_after_a_leading_delimiter() {
        let sample = [0, 0, 0, 2, 0x09, 0xf0, 0, 0, 0, 2, 0x65, 9];
        let annexb = avcc_to_annexb(&sample, 4, &[&[0x67, 1], &[0x68, 2]]).unwrap();
        assert_eq!(
            annexb,
            [
                0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 9
            ]
        );
        assert_eq!(leading_delimiter_len(&annexb), 6);
        assert_eq!(leading_delimiter_len(&annexb[6..]), 0);
        assert_eq!(leading_delimiter_len(&[0, 0, 1, 0x09, 0xf0]), 5);
    }

    #[test]
    fn the_nal_limit_does_not_depend_on_chunk_boundaries() {
        let nal = [0, 0, 1, 0x41, 1, 2, 3, 4, 5, 6, 7];
        let mut split = AnnexBReader::new(8);
        assert!(split.push(&nal).is_ok(), "an 8-byte NAL still waiting for its end");
        assert!(split.push(&[0, 0, 1, 0x09, 0xf0]).is_ok());
        let mut whole = AnnexBReader::new(8);
        assert!(whole.push(&[&nal[..], &[0, 0, 1, 0x09, 0xf0]].concat()).is_ok());
        let mut over = AnnexBReader::new(8);
        assert_eq!(over.push(&[&nal[..], &[8]].concat()), Err(H264Error::NalTooLarge));
        let mut junk = AnnexBReader::new(8);
        assert!(junk.push(&[7; 64]).is_ok(), "bytes before any start code are dropped");
    }

    #[test]
    fn a_completed_parameter_set_above_the_limit_is_rejected() {
        let mut reader = AnnexBReader::new(8);
        let mut data = vec![0, 0, 1, 0x67];
        data.extend_from_slice(&[1; 12]);
        data.extend_from_slice(&[0, 0, 1, 0x09, 0xf0]);
        assert_eq!(reader.push(&data), Err(H264Error::NalTooLarge));
    }

    #[test]
    fn one_large_chunk_of_small_units_is_within_the_limit() {
        let mut chunk = Vec::new();
        for picture in 0..4 {
            chunk.extend_from_slice(&[0, 0, 1, 0x09, 0xf0, 0, 0, 1, 0x41, picture]);
        }
        chunk.extend_from_slice(&[0, 0, 1, 0x09, 0xf0]);
        let mut reader = AnnexBReader::new(16);
        assert!(chunk.len() > 16);
        let units = reader.push(&chunk).unwrap();
        // The trailing delimiter stays pending until the next start code arrives.
        assert_eq!(units.len(), 3);
        assert_eq!(reader.finish().unwrap().len(), 1);
    }
}
