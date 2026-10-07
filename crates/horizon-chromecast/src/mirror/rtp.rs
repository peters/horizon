//! Cast Streaming RTP. Each encrypted frame is split over packets that carry
//! the RTP header and the Cast header: key frame flag, frame ID, packet ID,
//! last packet ID and the frame it depends on.

/// One Ethernet frame over IPv4 and UDP.
pub(crate) const MAX_PACKET: usize = 1500 - 20 - 8;
/// RTP header (12 bytes) plus the Cast header with a reference frame ID (7).
const HEADER: usize = 12 + 7;
const RTP_VERSION_2: u8 = 0x80;
const MARKER: u8 = 0x80;
const KEY_FRAME: u8 = 0x80;
const HAS_REFERENCE: u8 = 0x40;
/// Extension type for a new target playout delay, in milliseconds.
const PLAYOUT_DELAY_EXTENSION: u16 = 1;
const EXTENSION_SIZE_BITS: u32 = 10;

pub(crate) struct Frame<'a> {
    pub id: u32,
    /// The frame this one is decoded from; a key frame references itself.
    pub referenced: u32,
    pub keyframe: bool,
    pub rtp_timestamp: u32,
    /// Announces a new playout delay to the receiver.
    pub playout_delay_ms: Option<u16>,
    /// The encrypted frame.
    pub payload: &'a [u8],
}

/// The packets of one frame. Sequence numbers continue from `sequence`.
pub(crate) fn packetize(frame: &Frame, payload_type: u8, ssrc: u32, sequence: &mut u16) -> Vec<Vec<u8>> {
    let header = HEADER + if frame.playout_delay_ms.is_some() { 4 } else { 0 };
    let room = MAX_PACKET - header;
    let count = frame.payload.len().div_ceil(room).max(1);
    // Spread the payload evenly instead of ending on a sliver.
    let size = frame.payload.len().div_ceil(count).max(1);
    let slices: Vec<&[u8]> = if frame.payload.is_empty() {
        vec![&[]]
    } else {
        frame.payload.chunks(size).collect()
    };
    let last = u16::try_from(slices.len() - 1).unwrap_or(u16::MAX);
    slices
        .into_iter()
        .zip(0..=last)
        .map(|(slice, packet_id)| {
            let mut packet = Vec::with_capacity(header + slice.len());
            packet.push(RTP_VERSION_2);
            packet.push(if packet_id == last { MARKER } else { 0 } | payload_type);
            packet.extend_from_slice(&sequence.to_be_bytes());
            *sequence = sequence.wrapping_add(1);
            packet.extend_from_slice(&frame.rtp_timestamp.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());
            let extensions = u8::from(frame.playout_delay_ms.is_some());
            packet.push(if frame.keyframe { KEY_FRAME } else { 0 } | HAS_REFERENCE | extensions);
            packet.push(frame.id.to_be_bytes()[3]);
            packet.extend_from_slice(&packet_id.to_be_bytes());
            packet.extend_from_slice(&last.to_be_bytes());
            packet.push(frame.referenced.to_be_bytes()[3]);
            if let Some(delay) = frame.playout_delay_ms {
                packet.extend_from_slice(&((PLAYOUT_DELAY_EXTENSION << EXTENSION_SIZE_BITS) | 2).to_be_bytes());
                packet.extend_from_slice(&delay.to_be_bytes());
            }
            packet.extend_from_slice(slice);
            packet
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet_id(packet: &[u8]) -> Option<u16> {
        Some(u16::from_be_bytes(packet.get(14..16)?.try_into().ok()?))
    }

    fn frame(payload: &[u8], keyframe: bool, delay: Option<u16>) -> Frame<'_> {
        Frame {
            id: 0x1_0203,
            referenced: if keyframe { 0x1_0203 } else { 0x1_0202 },
            keyframe,
            rtp_timestamp: 0xdead_beef,
            playout_delay_ms: delay,
            payload,
        }
    }

    #[test]
    fn a_small_key_frame_is_one_marked_packet() {
        let mut sequence = 7;
        let packets = packetize(&frame(&[1, 2, 3], true, None), 0x60, 0x0102_0304, &mut sequence);
        assert_eq!(packets.len(), 1);
        assert_eq!(
            packets[0],
            [
                0x80,
                0x80 | 0x60,
                0,
                7,
                0xde,
                0xad,
                0xbe,
                0xef,
                1,
                2,
                3,
                4, // RTP
                0x80 | 0x40,
                0x03,
                0,
                0,
                0,
                0,
                0x03, // Cast: key, frame 3, packet 0 of 0, ref 3
                1,
                2,
                3,
            ]
        );
        assert_eq!(sequence, 8);
    }

    #[test]
    fn a_large_frame_splits_evenly_and_marks_only_the_last_packet() {
        let payload: Vec<u8> = (0..4000u32).map(|n| n.to_le_bytes()[0]).collect();
        let mut sequence = u16::MAX;
        let packets = packetize(&frame(&payload, false, None), 96, 1, &mut sequence);
        assert_eq!(packets.len(), 3);
        assert!(packets.iter().all(|p| p.len() <= MAX_PACKET));
        let rebuilt: Vec<u8> = packets.iter().flat_map(|p| p[HEADER..].to_vec()).collect();
        assert_eq!(rebuilt, payload);
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet[1] & MARKER != 0, index == 2);
            assert_eq!(packet[12], HAS_REFERENCE, "delta frame, no extensions");
            assert_eq!(packet_id(packet), Some(u16::try_from(index).unwrap()));
            assert_eq!(&packet[16..18], &[0, 2], "last packet ID");
            assert_eq!(packet[18], 0x02, "references the previous frame");
        }
        assert_eq!(&packets[0][2..4], &[0xff, 0xff]);
        assert_eq!(&packets[1][2..4], &[0, 0], "sequence numbers wrap");
    }

    #[test]
    fn a_playout_delay_change_rides_as_an_extension() {
        let mut sequence = 0;
        let packets = packetize(&frame(&[9], true, Some(120)), 96, 1, &mut sequence);
        assert_eq!(packets[0][12], 0x80 | 0x40 | 1);
        assert_eq!(&packets[0][19..23], &[0x04, 0x02, 0, 120]);
        assert_eq!(packets[0][23], 9);
    }
}
