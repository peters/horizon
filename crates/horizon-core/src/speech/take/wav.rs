//! The length fields of a recording that its recorder did not close.
//!
//! Every recorder of the documents writes the length of the recording into the
//! header of its file when the file ends: `pw-record`, `arecord`, `ffmpeg` and
//! `rec` all do that on `SIGINT` and on `SIGTERM`. A recorder that Horizon had to
//! kill writes no length: it leaves the two fields of a canonical `RIFF`/`WAVE`
//! header (the `RIFF` chunk size at the eighth byte and the `data` chunk size at
//! the 40th) at zero or at `0xFFFFFFFF`.
//!
//! A service reads such a header as a recording of no data, so the take would be
//! refused for a property of its container and not of its audio. [`close`] writes
//! the two fields from the size of the file for exactly that shape and for no
//! other: a header of another shape, such as one with a `fact` chunk, is the
//! business of its recorder, and Horizon sends it as it is.

/// The length of the canonical `RIFF`/`WAVE` header of PCM audio that this module
/// repairs, and the offset of its data chunk.
pub const CANONICAL_HEADER: usize = 44;

/// The value of a length field that a recorder never wrote.
const OPEN_LENGTH_MAX: u32 = u32::MAX;

/// The size of the `fmt ` chunk of a canonical PCM header.
const CANONICAL_FMT_SIZE: u16 = 16;

/// The audio format of linear PCM.
const PCM_FORMAT: u16 = 1;

/// The two length fields of a canonical header, and the bytes that decide the
/// shape of the header.
struct Fields {
    riff_size: usize,
    data_size: usize,
}

fn little_u16(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .and_then(|field| field.try_into().ok())
        .map(u16::from_le_bytes)
}

fn little_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .and_then(|field| field.try_into().ok())
        .map(u32::from_le_bytes)
}

fn has_marker(bytes: &[u8], at: usize, marker: &[u8; 4]) -> bool {
    bytes.get(at..at + 4).is_some_and(|field| field == marker)
}

/// Whether `header` is the canonical header of PCM audio, and the offsets of its
/// two length fields.
fn canonical(header: &[u8]) -> Option<Fields> {
    if header.len() < CANONICAL_HEADER
        || !has_marker(header, 0, b"RIFF")
        || !has_marker(header, 8, b"WAVE")
        || !has_marker(header, 12, b"fmt ")
        || !has_marker(header, 36, b"data")
    {
        return None;
    }
    if little_u32(header, 16) != Some(u32::from(CANONICAL_FMT_SIZE)) || little_u16(header, 20) != Some(PCM_FORMAT) {
        return None;
    }
    Some(Fields {
        riff_size: 4,
        data_size: 40,
    })
}

/// Whether `value` is a length that a recorder never wrote.
fn open(value: u32) -> bool {
    value == 0 || value == OPEN_LENGTH_MAX
}

/// Write the two length fields of a canonical PCM header from the size of the file,
/// and answer whether this header was a shape with an open length.
///
/// `header` holds the first [`CANONICAL_HEADER`] bytes of the file and `file_bytes`
/// is the size of the whole file. A file that is no longer than its header, and a
/// header of another shape, answer `false` and are left alone: the caller sends
/// such a file as it is.
pub fn close(header: &mut [u8], file_bytes: u64) -> bool {
    let Some(fields) = canonical(header) else {
        return false;
    };
    if file_bytes <= CANONICAL_HEADER as u64 {
        return false;
    }
    let riff = little_u32(header, fields.riff_size).unwrap_or(0);
    let data = little_u32(header, fields.data_size).unwrap_or(0);
    if !open(riff) && !open(data) {
        return false;
    }
    let Ok(data_size) = u32::try_from(file_bytes - CANONICAL_HEADER as u64) else {
        return false;
    };
    let Ok(riff_size) = u32::try_from(file_bytes - 8) else {
        return false;
    };
    let Some(riff_field) = header.get_mut(fields.riff_size..fields.riff_size + 4) else {
        return false;
    };
    riff_field.copy_from_slice(&riff_size.to_le_bytes());
    let Some(data_field) = header.get_mut(fields.data_size..fields.data_size + 4) else {
        return false;
    };
    data_field.copy_from_slice(&data_size.to_le_bytes());
    true
}

/// The canonical header of PCM audio whose data chunk holds `data_bytes` bytes, with
/// `length` written in both length fields, so a test builds the header of a file
/// that its recorder closed or never closed.
#[cfg(test)]
pub(super) fn header_of(data_bytes: u32, length: u32) -> Vec<u8> {
    let mut header = Vec::with_capacity(CANONICAL_HEADER);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&u32::from(CANONICAL_FMT_SIZE).to_le_bytes());
    header.extend_from_slice(&PCM_FORMAT.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&16_000u32.to_le_bytes());
    header.extend_from_slice(&32_000u32.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    assert_eq!(header.len(), CANONICAL_HEADER);
    header
}

#[cfg(test)]
mod tests {
    use super::{CANONICAL_HEADER, OPEN_LENGTH_MAX, close, header_of};

    fn lengths(header: &[u8]) -> (u32, u32) {
        let riff = u32::from_le_bytes(header[4..8].try_into().expect("the RIFF size"));
        let data = u32::from_le_bytes(header[40..44].try_into().expect("the data size"));
        (riff, data)
    }

    /// A header of `bytes` with the two length fields of `length`, as the start of a
    /// file of `file_bytes` bytes.
    fn closed(bytes: u32, length: u32, file_bytes: u64) -> (Vec<u8>, bool) {
        let mut header = header_of(bytes, length);
        let repaired = close(&mut header, file_bytes);
        (header, repaired)
    }

    #[test]
    fn a_header_that_its_recorder_never_closed_gets_the_length_of_the_file() {
        // A recorder that Horizon killed leaves zero, so the file of 44 + 1024 bytes
        // is a recording of 1024 bytes after the repair.
        let (header, repaired) = closed(0, 0, CANONICAL_HEADER as u64 + 1024);
        assert!(repaired);
        assert_eq!(lengths(&header), (1024 + CANONICAL_HEADER as u32 - 8, 1024));
        assert_eq!(lengths(&header), (1060, 1024));
    }

    #[test]
    fn a_header_that_says_the_longest_length_is_closed_the_same_way() {
        let (header, repaired) = closed(OPEN_LENGTH_MAX, OPEN_LENGTH_MAX, CANONICAL_HEADER as u64 + 512);
        assert!(repaired);
        assert_eq!(lengths(&header), (512 + CANONICAL_HEADER as u32 - 8, 512));
    }

    #[test]
    fn one_open_field_of_a_closed_recording_is_closed_with_the_other() {
        let (header, repaired) = closed(0, 1024, CANONICAL_HEADER as u64 + 1024);
        assert!(repaired);
        assert_eq!(lengths(&header), (1060, 1024));
    }

    #[test]
    fn a_header_that_its_recorder_closed_is_left_as_it_is() {
        let mut header = header_of(1024, 1060);
        let before = header.clone();
        assert!(!close(&mut header, CANONICAL_HEADER as u64 + 1024));
        assert_eq!(header, before, "a closed header is the recorder's own bytes");
    }

    #[test]
    fn a_file_that_is_no_longer_than_its_header_is_left_as_it_is() {
        let mut header = header_of(0, 0);
        let before = header.clone();
        assert!(!close(&mut header, CANONICAL_HEADER as u64));
        assert_eq!(header, before);
    }

    #[test]
    fn a_header_of_another_shape_is_sent_as_it_is() {
        // The `fmt ` chunk of 18 bytes names a `fact` chunk that a canonical header
        // does not have, so the length fields of this file are not Horizon's to fix.
        let mut extended = header_of(0, 0);
        extended[16..20].copy_from_slice(&18u32.to_le_bytes());
        let before = extended.clone();
        assert!(!close(&mut extended, CANONICAL_HEADER as u64 + 1024));
        assert_eq!(extended, before);

        // A file of a recorder that writes no `RIFF` header at all, such as a stream
        // of the audio bytes, is sent as it is too.
        let mut raw = vec![0u8; CANONICAL_HEADER + 1024];
        let before = raw.clone();
        let raw_bytes = raw.len() as u64;
        assert!(!close(&mut raw, raw_bytes));
        assert_eq!(raw, before);

        // A header that is shorter than the canonical one holds no field to fix.
        let mut short = header_of(0, 0);
        short.truncate(CANONICAL_HEADER - 4);
        let before = short.clone();
        assert!(!close(&mut short, CANONICAL_HEADER as u64 + 1024));
        assert_eq!(short, before);
    }
}
