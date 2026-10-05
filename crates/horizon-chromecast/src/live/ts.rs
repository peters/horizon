//! Minimal MPEG-TS muxer for one H.264 elementary stream.
pub(crate) const PACKET_SIZE: usize = 188;

const SYNC_BYTE: u8 = 0x47;
const PID_PAT: u16 = 0x0000;
const PID_PMT: u16 = 0x1000;
pub(crate) const PID_VIDEO: u16 = 0x0100;
const STREAM_TYPE_H264: u8 = 0x1b;
const STREAM_ID_VIDEO: u8 = 0xe0;
const PROGRAM_NUMBER: u16 = 1;
/// Presentation trails the clock reference so the receiver has decode headroom.
const PCR_LEAD: u64 = 9_000;
const PAYLOAD_SIZE: usize = PACKET_SIZE - 4;

/// Fields are the continuity counters of each PID.
#[derive(Default)]
pub(crate) struct TsMuxer {
    pat: u8,
    pmt: u8,
    video: u8,
}

impl TsMuxer {
    /// Writes PAT and PMT. Every segment starts with them so it decodes alone.
    pub(crate) fn write_tables(&mut self, out: &mut Vec<u8>) {
        let pat = psi_section(0x00, 0x0001, &program_association());
        write_psi(out, PID_PAT, &mut self.pat, &pat);
        let pmt = psi_section(0x02, PROGRAM_NUMBER, &program_map());
        write_psi(out, PID_PMT, &mut self.pmt, &pmt);
    }

    /// Writes one Annex B access unit with a PES header and a PCR on its first packet.
    pub(crate) fn write_access_unit(&mut self, out: &mut Vec<u8>, annexb: &[u8], clock_90k: u64, keyframe: bool) {
        let mut pes = Vec::with_capacity(annexb.len() + 14);
        pes.extend_from_slice(&[0x00, 0x00, 0x01, STREAM_ID_VIDEO, 0x00, 0x00, 0x80, 0x80, 0x05]);
        pes.extend_from_slice(&timestamp(clock_90k + PCR_LEAD));
        pes.extend_from_slice(annexb);
        let mut remaining = pes.as_slice();
        let mut first = true;
        while !remaining.is_empty() {
            let pcr = first.then_some(clock_90k);
            let adaptation_flags = if first && keyframe {
                0x50
            } else if first {
                0x10
            } else {
                0x00
            };
            let mandatory = if pcr.is_some() { 8 } else { 0 };
            let take = remaining.len().min(PAYLOAD_SIZE - mandatory);
            let adaptation = (PAYLOAD_SIZE - take).max(mandatory);
            write_header(out, PID_VIDEO, first, adaptation > 0, &mut self.video);
            write_adaptation(out, adaptation, adaptation_flags, pcr);
            out.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            first = false;
        }
    }
}

fn write_header(out: &mut Vec<u8>, pid: u16, unit_start: bool, adaptation: bool, continuity: &mut u8) {
    let [pid_high, pid_low] = pid.to_be_bytes();
    out.push(SYNC_BYTE);
    out.push(u8::from(unit_start) << 6 | (pid_high & 0x1f));
    out.push(pid_low);
    let control = if adaptation { 0x30 } else { 0x10 };
    out.push(control | *continuity);
    *continuity = (*continuity + 1) & 0x0f;
}

/// `size` counts the adaptation field including its length byte.
fn write_adaptation(out: &mut Vec<u8>, size: usize, flags: u8, pcr: Option<u64>) {
    if size == 0 {
        return;
    }
    let start = out.len();
    out.push(u8::try_from(size - 1).unwrap_or(u8::MAX));
    if size > 1 {
        out.push(flags);
        if let Some(pcr) = pcr {
            let base = pcr & ((1 << 33) - 1);
            let bytes = (base << 15 | 0x3f << 9).to_be_bytes();
            out.extend_from_slice(&bytes[2..]);
        }
    }
    out.resize(start + size, 0xff);
}

fn write_psi(out: &mut Vec<u8>, pid: u16, continuity: &mut u8, section: &[u8]) {
    write_header(out, pid, true, false, continuity);
    let start = out.len();
    out.push(0x00);
    out.extend_from_slice(section);
    out.resize(start + PAYLOAD_SIZE, 0xff);
}

fn program_association() -> Vec<u8> {
    let mut body = PROGRAM_NUMBER.to_be_bytes().to_vec();
    body.extend_from_slice(&(0xe000 | PID_PMT).to_be_bytes());
    body
}

fn program_map() -> Vec<u8> {
    let mut body = (0xe000 | PID_VIDEO).to_be_bytes().to_vec();
    body.extend_from_slice(&[0xf0, 0x00, STREAM_TYPE_H264]);
    body.extend_from_slice(&(0xe000 | PID_VIDEO).to_be_bytes());
    body.extend_from_slice(&[0xf0, 0x00]);
    body
}

fn psi_section(table_id: u8, id: u16, body: &[u8]) -> Vec<u8> {
    let length = u16::try_from(body.len() + 9).unwrap_or(u16::MAX);
    let mut section = vec![table_id];
    section.extend_from_slice(&(0xb000 | length).to_be_bytes());
    section.extend_from_slice(&id.to_be_bytes());
    section.extend_from_slice(&[0xc1, 0x00, 0x00]);
    section.extend_from_slice(body);
    section.extend_from_slice(&crc32_mpeg2(&section).to_be_bytes());
    section
}

/// PES timestamp with the PTS-only `0010` prefix and marker bits.
fn timestamp(value: u64) -> [u8; 5] {
    let value = value & ((1 << 33) - 1);
    let byte = |shift: u32| (value >> shift).to_le_bytes()[0];
    [
        0x21 | (byte(29) & 0x0e),
        byte(22),
        (byte(14) & 0xfe) | 1,
        byte(7),
        (byte(0) << 1) | 1,
    ]
}

pub(crate) fn crc32_mpeg2(data: &[u8]) -> u32 {
    data.iter().fold(u32::MAX, |crc, &byte| {
        (0..8).fold(crc ^ (u32::from(byte) << 24), |crc, _| {
            if crc & 0x8000_0000 == 0 {
                crc << 1
            } else {
                crc << 1 ^ 0x04c1_1db7
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packets(data: &[u8]) -> Vec<&[u8]> {
        assert_eq!(data.len() % PACKET_SIZE, 0);
        data.chunks(PACKET_SIZE).collect()
    }

    fn pid(packet: &[u8]) -> u16 {
        u16::from_be_bytes([packet[1] & 0x1f, packet[2]])
    }

    #[test]
    fn crc_matches_reference_vector() {
        assert_eq!(crc32_mpeg2(b"123456789"), 0x0376_e6e7);
    }

    #[test]
    fn tables_carry_valid_sections() {
        let mut out = Vec::new();
        TsMuxer::default().write_tables(&mut out);
        for packet in packets(&out) {
            assert_eq!(packet[0], SYNC_BYTE);
            let section_length = usize::from(u16::from_be_bytes([packet[6], packet[7]]) & 0x0fff);
            let section = &packet[5..8 + section_length];
            assert_eq!(crc32_mpeg2(section), 0, "CRC over a section including its CRC is zero");
        }
        assert_eq!(pid(&out[PACKET_SIZE..]), PID_PMT);
    }

    #[test]
    fn access_units_split_into_packets_with_pcr_and_continuity() {
        let mut muxer = TsMuxer::default();
        let mut out = Vec::new();
        for (size, clock) in [(10, 0), (183, 3_000), (184, 6_000), (5_000, 9_000)] {
            let unit: Vec<u8> = (0..size).map(|i| u8::try_from(i % 251).unwrap()).collect();
            let before = out.len();
            muxer.write_access_unit(&mut out, &unit, clock, clock == 0);
            let written = &out[before..];
            let first = &written[..PACKET_SIZE];
            assert_eq!(first[1] & 0x40, 0x40, "payload unit start");
            assert_eq!(first[5] & 0x10, 0x10, "PCR flag");
            let pcr = u64::from_be_bytes([0, 0, first[6], first[7], first[8], first[9], first[10], first[11]]) >> 15;
            assert_eq!(pcr, clock);
            let payload_start = 5 + usize::from(first[4]);
            assert_eq!(&first[payload_start..payload_start + 4], &[0, 0, 1, STREAM_ID_VIDEO]);
            let mut payload = Vec::new();
            for packet in packets(written) {
                let start = if packet[3] & 0x20 != 0 {
                    5 + usize::from(packet[4])
                } else {
                    4
                };
                payload.extend_from_slice(&packet[start..]);
            }
            assert_eq!(&payload[payload.len() - size..], unit.as_slice());
        }
        let counters: Vec<u8> = packets(&out).iter().map(|p| p[3] & 0x0f).collect();
        for pair in counters.windows(2) {
            assert_eq!(pair[1], (pair[0] + 1) & 0x0f);
        }
        assert_eq!(out[5] & 0x40, 0x40, "keyframe sets random access");
    }

    #[test]
    fn timestamps_round_trip() {
        let value = 0x1_2345_6789;
        let b = timestamp(value);
        let decoded = (u64::from(b[0] & 0x0e) << 29)
            | (u64::from(b[1]) << 22)
            | (u64::from(b[2] >> 1) << 15)
            | (u64::from(b[3]) << 7)
            | u64::from(b[4] >> 1);
        assert_eq!(decoded, value);
    }
}
