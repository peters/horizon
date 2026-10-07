//! Cast Streaming RTCP: the sender reports that map RTP timestamps to wall
//! clock time, and the receiver's feedback (picture loss, and Cast feedback
//! with the newest complete frame and the packets still missing).
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION_2: u8 = 0x80;
const SENDER_REPORT: u8 = 200;
const PAYLOAD_FEEDBACK: u8 = 206;
const PICTURE_LOSS: u8 = 1;
const APPLICATION_FEEDBACK: u8 = 15;
const CAST: &[u8; 4] = b"CAST";
/// Seconds from the NTP epoch (1900) to the Unix epoch (1970).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
/// A loss field's packet ID meaning every packet of the frame is missing.
pub(crate) const ALL_PACKETS: u16 = 0xffff;

/// Wall clock time as a 64-bit NTP timestamp.
pub(crate) fn ntp_now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let fraction = (u64::from(since.subsec_nanos()) << 32) / 1_000_000_000;
    ((since.as_secs() + NTP_UNIX_OFFSET) << 32) | fraction
}

pub(crate) struct SenderReport {
    pub ssrc: u32,
    pub ntp: u64,
    pub rtp_timestamp: u32,
    pub packets: u32,
    pub octets: u32,
}

impl SenderReport {
    pub(crate) fn encode(&self) -> [u8; 28] {
        let mut out = [0; 28];
        out[0] = VERSION_2;
        out[1] = SENDER_REPORT;
        // Length in 32-bit words, minus one.
        out[2..4].copy_from_slice(&6u16.to_be_bytes());
        out[4..8].copy_from_slice(&self.ssrc.to_be_bytes());
        out[8..16].copy_from_slice(&self.ntp.to_be_bytes());
        out[16..20].copy_from_slice(&self.rtp_timestamp.to_be_bytes());
        out[20..24].copy_from_slice(&self.packets.to_be_bytes());
        out[24..28].copy_from_slice(&self.octets.to_be_bytes());
        out
    }
}

/// Packets of one frame the receiver reports missing: `packet`, and each
/// following packet whose bit is set in `bitmask` (bit 0 is `packet + 1`).
/// [`ALL_PACKETS`] asks for the whole frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Loss {
    /// The frame ID's lowest 8 bits.
    pub frame: u8,
    pub packet: u16,
    pub bitmask: u8,
}

impl Loss {
    /// Missing packet IDs, given the frame's last packet ID.
    pub(crate) fn packets(self, last: u16) -> Vec<u16> {
        if self.packet == ALL_PACKETS {
            return (0..=last).collect();
        }
        std::iter::once(self.packet)
            .chain(
                (0..8u16)
                    .filter(|bit| self.bitmask & (1 << bit) != 0)
                    .map(|bit| self.packet.saturating_add(bit + 1)),
            )
            .filter(|packet| *packet <= last)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Feedback {
    /// The receiver cannot decode until the next key frame.
    PictureLoss { media_ssrc: u32 },
    Cast {
        media_ssrc: u32,
        /// The newest frame (lowest 8 bits) received with every frame before it.
        checkpoint: u8,
        target_delay_ms: u16,
        losses: Vec<Loss>,
    },
}

/// Feedback in a compound RTCP packet. Report blocks and extended reports
/// carry nothing the sender acts on and are skipped.
pub(crate) fn parse(packet: &[u8]) -> Vec<Feedback> {
    let mut found = Vec::new();
    let mut rest = packet;
    while rest.len() >= 4 && rest[0] & 0xc0 == VERSION_2 {
        let size = (usize::from(u16::from_be_bytes([rest[2], rest[3]])) + 1) * 4;
        let Some(body) = rest.get(..size) else {
            break;
        };
        if body[1] == PAYLOAD_FEEDBACK && body.len() >= 12 {
            let media_ssrc = u32::from_be_bytes([body[8], body[9], body[10], body[11]]);
            match body[0] & 0x1f {
                PICTURE_LOSS => found.push(Feedback::PictureLoss { media_ssrc }),
                APPLICATION_FEEDBACK if body.get(12..16) == Some(CAST) && body.len() >= 20 => {
                    let count = usize::from(body[17]);
                    let losses = body[20..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .take(count)
                        .map(|field| Loss {
                            frame: field[0],
                            packet: u16::from_be_bytes([field[1], field[2]]),
                            bitmask: field[3],
                        })
                        .collect();
                    found.push(Feedback::Cast {
                        media_ssrc,
                        checkpoint: body[16],
                        target_delay_ms: u16::from_be_bytes([body[18], body[19]]),
                        losses,
                    });
                }
                _ => {}
            }
        }
        rest = &rest[size..];
    }
    found
}

/// RTCP rather than RTP: payload types 200..=207 sit where RTP has its marker bit and type.
pub(crate) fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 4 && packet[0] & 0xc0 == VERSION_2 && (200..=207).contains(&packet[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_report_has_the_rfc_3550_layout() {
        let report = SenderReport {
            ssrc: 0x0102_0304,
            ntp: 0x1122_3344_5566_7788,
            rtp_timestamp: 0x99aa_bbcc,
            packets: 5,
            octets: 6000,
        }
        .encode();
        assert_eq!(&report[..4], &[0x80, 200, 0, 6]);
        assert_eq!(&report[4..8], &[1, 2, 3, 4]);
        assert_eq!(&report[8..16], &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        assert_eq!(&report[16..20], &[0x99, 0xaa, 0xbb, 0xcc]);
        assert_eq!(&report[20..28], &[0, 0, 0, 5, 0, 0, 0x17, 0x70]);
        assert!(is_rtcp(&report));
    }

    #[test]
    fn ntp_time_counts_from_1900() {
        let seconds = ntp_now() >> 32;
        // 2020-01-01 is 3_786_825_600 seconds after the NTP epoch.
        assert!(seconds > 3_786_825_600, "{seconds}");
    }

    #[test]
    fn compound_feedback_yields_picture_loss_and_cast_losses() {
        let receiver_report = [0x80, 201, 0, 1, 0, 0, 0, 9];
        let picture_loss = [0x81, 206, 0, 2, 0, 0, 0, 9, 0, 0, 0, 7];
        let mut cast = vec![0x8f, 206, 0, 6, 0, 0, 0, 9, 0, 0, 0, 7];
        cast.extend_from_slice(b"CAST");
        cast.extend_from_slice(&[0x2a, 2, 0, 100]);
        cast.extend_from_slice(&[0x2b, 0, 3, 0b101]);
        cast.extend_from_slice(&[0x2c, 0xff, 0xff, 0]);
        let compound: Vec<u8> = [&receiver_report[..], &picture_loss, &cast].concat();
        assert!(is_rtcp(&compound));
        assert_eq!(
            parse(&compound),
            [
                Feedback::PictureLoss { media_ssrc: 7 },
                Feedback::Cast {
                    media_ssrc: 7,
                    checkpoint: 0x2a,
                    target_delay_ms: 100,
                    losses: vec![
                        Loss {
                            frame: 0x2b,
                            packet: 3,
                            bitmask: 0b101
                        },
                        Loss {
                            frame: 0x2c,
                            packet: ALL_PACKETS,
                            bitmask: 0
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn losses_expand_to_packet_ids_within_the_frame() {
        let some = Loss {
            frame: 1,
            packet: 3,
            bitmask: 0b1000_0101,
        };
        assert_eq!(some.packets(20), [3, 4, 6, 11]);
        assert_eq!(some.packets(5), [3, 4]);
        let all = Loss {
            frame: 1,
            packet: ALL_PACKETS,
            bitmask: 0,
        };
        assert_eq!(all.packets(2), [0, 1, 2]);
    }

    #[test]
    fn truncated_or_foreign_packets_yield_nothing() {
        assert!(parse(&[0x8f, 206, 0, 9, 0, 0]).is_empty());
        assert!(parse(&[0x00, 206, 0, 0]).is_empty());
        assert!(!is_rtcp(&[0x80, 96, 0, 1]));
    }
}
