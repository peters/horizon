//! Minimal one-track AV1 `WebM` muxer.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

const TIMESTAMP_SCALE: u64 = 1_000_000;
const CLUSTER_MAX_MS: u64 = 1_000;

pub(super) struct WebmMuxer {
    file: File,
    segment_payload_start: u64,
    duration_offset: u64,
    cues: Vec<CuePoint>,
    cluster: ClusterBuffer,
    last_timestamp_ms: u64,
}

struct CuePoint {
    time_ms: u64,
    cluster_position: u64,
}

struct ClusterBuffer {
    timestamp_ms: u64,
    payload: Vec<u8>,
}

impl WebmMuxer {
    pub(super) fn create(mut file: File, width: u32, height: u32, codec_private: &[u8]) -> io::Result<Self> {
        write_ebml_header(&mut file)?;
        file.write_all(&[0x18, 0x53, 0x80, 0x67])?;
        file.write_all(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?;
        let segment_payload_start = file.stream_position()?;
        let duration_offset = write_info(&mut file)?;
        write_tracks(&mut file, width, height, codec_private)?;
        Ok(Self {
            file,
            segment_payload_start,
            duration_offset,
            cues: Vec::new(),
            cluster: ClusterBuffer {
                timestamp_ms: 0,
                payload: Vec::new(),
            },
            last_timestamp_ms: 0,
        })
    }

    pub(super) fn write_frame(&mut self, timestamp_ms: u64, keyframe: bool, data: &[u8]) -> io::Result<()> {
        if self.cluster.payload.is_empty() {
            self.cluster.timestamp_ms = timestamp_ms;
        } else if timestamp_ms.saturating_sub(self.cluster.timestamp_ms) >= CLUSTER_MAX_MS {
            self.flush_cluster()?;
            self.cluster.timestamp_ms = timestamp_ms;
        }
        let relative = i16::try_from(timestamp_ms.saturating_sub(self.cluster.timestamp_ms)).unwrap_or(i16::MAX);
        self.cluster
            .payload
            .extend_from_slice(&simple_block(1, relative, keyframe, data));
        self.last_timestamp_ms = self.last_timestamp_ms.max(timestamp_ms);
        Ok(())
    }

    pub(super) fn bytes_written(&mut self) -> io::Result<u64> {
        self.file.stream_position()
    }

    pub(super) fn would_exceed(&mut self, extra: u64, max_file_bytes: u64) -> io::Result<bool> {
        const FINALIZE_RESERVE: u64 = 8 * 1024;
        const CLUSTER_HEADER_SLACK: u64 = 32;
        let position = self.file.stream_position()?;
        let buffered = u64::try_from(self.cluster.payload.len()).unwrap_or(u64::MAX);
        Ok(position
            .saturating_add(buffered)
            .saturating_add(CLUSTER_HEADER_SLACK)
            .saturating_add(extra)
            .saturating_add(FINALIZE_RESERVE)
            > max_file_bytes)
    }

    pub(super) fn finish(mut self) -> io::Result<u64> {
        self.flush_cluster()?;
        write_cues(&mut self.file, &self.cues)?;
        let end = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(self.duration_offset))?;
        self.file.write_all(&duration_millis(self.last_timestamp_ms))?;
        self.file.flush()?;
        Ok(end)
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        if self.cluster.payload.is_empty() {
            return Ok(());
        }
        let position = self.file.stream_position()? - self.segment_payload_start;
        let mut payload = Vec::new();
        append_element(&mut payload, &[0xE7], &encode_uint(self.cluster.timestamp_ms));
        payload.extend_from_slice(&self.cluster.payload);
        write_element(&mut self.file, &[0x1F, 0x43, 0xB6, 0x75], &payload)?;
        self.cues.push(CuePoint {
            time_ms: self.cluster.timestamp_ms,
            cluster_position: position,
        });
        self.cluster.payload.clear();
        Ok(())
    }
}

fn write_ebml_header(out: &mut File) -> io::Result<()> {
    let mut payload = Vec::new();
    append_element(&mut payload, &[0x42, 0x86], &encode_uint(1));
    append_element(&mut payload, &[0x42, 0xF7], &encode_uint(1));
    append_element(&mut payload, &[0x42, 0xF2], &encode_uint(4));
    append_element(&mut payload, &[0x42, 0xF3], &encode_uint(8));
    append_element(&mut payload, &[0x42, 0x82], b"webm");
    append_element(&mut payload, &[0x42, 0x87], &encode_uint(4));
    append_element(&mut payload, &[0x42, 0x85], &encode_uint(2));
    write_element(out, &[0x1A, 0x45, 0xDF, 0xA3], &payload)
}

fn write_info(out: &mut File) -> io::Result<u64> {
    let mut payload = Vec::new();
    append_element(&mut payload, &[0x2A, 0xD7, 0xB1], &encode_uint(TIMESTAMP_SCALE));
    append_element(&mut payload, &[0x4D, 0x80], b"horizon");
    append_element(&mut payload, &[0x57, 0x41], b"horizon");
    payload.extend_from_slice(&[0x44, 0x89]);
    payload.extend_from_slice(&encode_vint(8));
    let header_end = {
        let mut header = Vec::new();
        header.extend_from_slice(&[0x15, 0x49, 0xA9, 0x66]);
        header.extend_from_slice(&encode_vint((payload.len() + 8) as u64));
        out.write_all(&header)?;
        out.stream_position()? + payload.len() as u64
    };
    out.write_all(&payload)?;
    out.write_all(&0f64.to_be_bytes())?;
    Ok(header_end)
}

fn write_tracks(out: &mut File, width: u32, height: u32, codec_private: &[u8]) -> io::Result<()> {
    let mut video = Vec::new();
    append_element(&mut video, &[0xB0], &encode_uint(u64::from(width)));
    append_element(&mut video, &[0xBA], &encode_uint(u64::from(height)));
    let mut entry = Vec::new();
    append_element(&mut entry, &[0xD7], &encode_uint(1));
    append_element(&mut entry, &[0x73, 0xC5], &encode_uint(1));
    append_element(&mut entry, &[0x83], &encode_uint(1));
    append_element(&mut entry, &[0x86], b"V_AV1");
    if !codec_private.is_empty() {
        append_element(&mut entry, &[0x63, 0xA2], codec_private);
    }
    append_element(&mut entry, &[0xE0], &video);
    let mut tracks = Vec::new();
    append_element(&mut tracks, &[0xAE], &entry);
    write_element(out, &[0x16, 0x54, 0xAE, 0x6B], &tracks)
}

fn write_cues(out: &mut File, cues: &[CuePoint]) -> io::Result<()> {
    let mut payload = Vec::new();
    for cue in cues {
        let mut positions = Vec::new();
        append_element(&mut positions, &[0xF7], &encode_uint(1));
        append_element(&mut positions, &[0xF1], &encode_uint(cue.cluster_position));
        let mut point = Vec::new();
        append_element(&mut point, &[0xB3], &encode_uint(cue.time_ms));
        append_element(&mut point, &[0xB7], &positions);
        append_element(&mut payload, &[0xBB], &point);
    }
    write_element(out, &[0x1C, 0x53, 0xBB, 0x6B], &payload)
}

fn simple_block(track: u64, relative_ms: i16, keyframe: bool, data: &[u8]) -> Vec<u8> {
    let mut payload = encode_vint(track);
    payload.extend_from_slice(&relative_ms.to_be_bytes());
    payload.push(if keyframe { 0x80 } else { 0x00 });
    payload.extend_from_slice(data);
    let mut block = Vec::new();
    append_element(&mut block, &[0xA3], &payload);
    block
}

fn append_element(out: &mut Vec<u8>, id: &[u8], payload: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&encode_vint(payload.len() as u64));
    out.extend_from_slice(payload);
}

fn write_element(out: &mut File, id: &[u8], payload: &[u8]) -> io::Result<()> {
    out.write_all(id)?;
    out.write_all(&encode_vint(payload.len() as u64))?;
    out.write_all(payload)
}

fn encode_uint(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len() - 1);
    bytes[start..].to_vec()
}

fn encode_vint(value: u64) -> Vec<u8> {
    for width in 1_u8..=8 {
        let value_bits = 7_u32.saturating_mul(u32::from(width));
        let unknown = (1_u64 << value_bits) - 1;
        if value < unknown {
            let marker = 1_u64 << value_bits;
            let encoded = value | marker;
            let width = usize::from(width);
            return encoded.to_be_bytes()[8 - width..].to_vec();
        }
    }
    vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE]
}

fn duration_millis(timestamp_ms: u64) -> [u8; 8] {
    f64::from(u32::try_from(timestamp_ms.min(u64::from(u32::MAX))).unwrap_or(u32::MAX)).to_be_bytes()
}

#[must_use]
pub(super) fn av1_codec_private(sequence_header_obu: &[u8]) -> Vec<u8> {
    let mut record = vec![
        0x81, // marker=1, version=1
        0x1F, // profile 0, level 31 (unspecified)
        0x0C, // 4:2:0 8-bit
        0x00,
    ];
    record.extend_from_slice(sequence_header_obu);
    record
}

#[must_use]
pub(super) fn sequence_header_obu(packet: &[u8]) -> Vec<u8> {
    let mut offset = 0;
    while offset < packet.len() {
        let Some((obu, next)) = next_obu(packet, offset) else {
            break;
        };
        if obu_type(packet[offset]) == 1 {
            return packet[offset..next.min(packet.len())].to_vec();
        }
        let _ = obu;
        offset = next;
    }
    Vec::new()
}

fn obu_type(header: u8) -> u8 {
    (header >> 3) & 0x0F
}

fn next_obu(packet: &[u8], offset: usize) -> Option<(&[u8], usize)> {
    let header = *packet.get(offset)?;
    if header & 0x80 != 0 {
        return None;
    }
    let extension = header & 0x04 != 0;
    let has_size = header & 0x02 != 0;
    let mut cursor = offset + 1;
    if extension {
        cursor = cursor.checked_add(1)?;
    }
    if has_size {
        let (size, size_len) = read_leb128(packet, cursor)?;
        cursor = cursor.checked_add(size_len)?;
        let end = cursor.checked_add(size)?;
        packet.get(offset..end).map(|obu| (obu, end))
    } else {
        Some((packet.get(offset..)?, packet.len()))
    }
}

fn read_leb128(data: &[u8], offset: usize) -> Option<(usize, usize)> {
    let mut value = 0_u64;
    for index in 0..8 {
        let byte = *data.get(offset + index)?;
        value |= u64::from(byte & 0x7F) << (index * 7);
        if byte & 0x80 == 0 {
            return usize::try_from(value).ok().map(|size| (size, index + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;

    #[test]
    fn muxer_writes_ebml_webm_header_and_duration() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let path = root.path().join("clip.webm");
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|error| panic!("open muxer file: {error}"));
        let mut muxer = WebmMuxer::create(file, 64, 64, &[0x81, 0x1F, 0x0C, 0x00])
            .unwrap_or_else(|error| panic!("create muxer: {error}"));
        muxer
            .write_frame(0, true, &[1, 2, 3, 4])
            .unwrap_or_else(|error| panic!("write frame: {error}"));
        muxer
            .write_frame(100, false, &[5, 6, 7, 8])
            .unwrap_or_else(|error| panic!("write frame: {error}"));
        muxer.finish().unwrap_or_else(|error| panic!("finish muxer: {error}"));
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("read muxer file: {error}"));
        assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);
        assert!(bytes.windows(4).any(|window| window == b"webm"));
        assert!(bytes.windows(5).any(|window| window == b"V_AV1"));
        assert!(bytes.len() > 64);
        assert_ne!(encode_vint(127), vec![0xFF]);
        assert_eq!(encode_vint(127), vec![0x40, 0x7F]);
        assert_ne!(encode_vint(16_383), vec![0x7F, 0xFF]);
    }
}
