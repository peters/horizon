//! Cast Streaming, the protocol receivers use to mirror a screen. Frames go
//! over UDP as encrypted RTP with a playout delay of about a tenth of a second,
//! instead of through a media player's buffer.
mod crypto;
mod offer;
mod rtcp;
mod rtp;
mod sender;

pub(crate) use offer::{Answer, NS_WEBRTC, answer, offer};
pub(crate) use sender::Sender;

use crate::{AudioFormat, Error, Result, live::mp4::audio_specific_config};
use offer::{Kind, StreamOffer};
use ring::rand::{SecureRandom, SystemRandom};
use sender::StreamSetup;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

/// Application id of the receiver that plays screen mirroring sessions.
pub const MIRRORING_RECEIVER: &str = "0F5096E8";

/// Pushed frames go to the session's sender once the receiver has answered.
pub(crate) struct MirrorStream {
    audio: Option<AudioFormat>,
    sender: Mutex<Option<Arc<Sender>>>,
}

/// One OFFER: what it proposes, and the RTP clock offsets to send with.
pub(crate) struct Proposal {
    pub streams: Vec<StreamOffer>,
    rtp_offsets: Vec<u32>,
}

impl MirrorStream {
    pub(crate) fn new(audio: Option<AudioFormat>) -> Self {
        Self {
            audio,
            sender: Mutex::new(None),
        }
    }

    pub(crate) fn push_video(&self, annexb: &[u8], pts: Duration, keyframe: bool) {
        if let Some(sender) = self.sender() {
            sender.send_video(annexb, pts, keyframe);
        }
    }

    /// Receivers decode mirrored AAC only with an ADTS header on each frame.
    pub(crate) fn push_audio(&self, frame: &[u8], pts: Duration) {
        if let (Some(sender), Some(framed)) = (self.sender(), self.audio.and_then(|format| adts(format, frame))) {
            sender.send_audio(&framed, pts);
        }
    }

    /// The receiver has no picture to build on until the next key frame.
    pub(crate) fn wants_keyframe(&self) -> bool {
        self.sender().is_some_and(|sender| sender.wants_keyframe())
    }

    pub(crate) fn sender(&self) -> Option<Arc<Sender>> {
        self.lock().clone()
    }

    /// Fresh streams with random SSRCs and keys for one OFFER.
    pub(crate) fn propose(&self) -> Result<Proposal> {
        let random = SystemRandom::new();
        let fill = |bytes: &mut [u8]| random.fill(bytes).map_err(|_| Error::Protocol("no random numbers"));
        let mut kinds = vec![Kind::Video];
        if let Some(format) = self.audio {
            kinds.push(Kind::Audio {
                sample_rate: format.sample_rate,
                channels: format.channels,
            });
        }
        let mut streams = Vec::new();
        let mut rtp_offsets = Vec::new();
        for (index, kind) in (0..).zip(kinds) {
            let (mut ssrc, mut offset, mut key, mut iv_mask) = ([0; 4], [0; 4], [0; 16], [0; 16]);
            fill(&mut ssrc)?;
            fill(&mut offset)?;
            fill(&mut key)?;
            fill(&mut iv_mask)?;
            streams.push(StreamOffer {
                index,
                kind,
                // Keep SSRCs positive for receivers that parse them as signed.
                ssrc: u32::from_be_bytes(ssrc) >> 1,
                key,
                iv_mask,
            });
            rtp_offsets.push(u32::from_be_bytes(offset));
        }
        Ok(Proposal { streams, rtp_offsets })
    }

    /// Starts sending to the receiver at `address` as it answered `proposal`,
    /// keeping frames for resending well past `playout_delay`.
    /// # Errors
    /// Fails when the receiver accepted no video stream, or no UDP socket opens.
    pub(crate) fn connect(
        &self,
        address: SocketAddr,
        proposal: &Proposal,
        answer: &Answer,
        playout_delay: Duration,
    ) -> Result<Arc<Sender>> {
        let accepted = |index: u32| answer.accepted.contains(&index);
        let mut setups = Vec::new();
        let mut offsets = Vec::new();
        for (stream, offset) in proposal.streams.iter().zip(&proposal.rtp_offsets) {
            if !accepted(stream.index) {
                if stream.kind == Kind::Video {
                    return Err(Error::Rejected {
                        kind: "ANSWER".to_owned(),
                        reason: Some("the receiver accepted no video stream".to_owned()),
                    });
                }
                continue;
            }
            setups.push(StreamSetup {
                ssrc: stream.ssrc,
                payload_type: stream.payload_type(),
                clock_rate: stream.clock_rate(),
                cipher: crypto::FrameCipher::new(stream.key, stream.iv_mask),
                independent_frames: stream.kind != Kind::Video,
            });
            offsets.push(*offset);
        }
        let sender = Arc::new(Sender::start(
            udp_target(address, answer.udp_port),
            setups,
            &offsets,
            resend_history(playout_delay),
        )?);
        *self.lock() = Some(sender.clone());
        Ok(sender)
    }

    /// Stops sending; the session's sender closes once its last user lets go.
    pub(crate) fn disconnect(&self) {
        // Dropped outside the lock: closing waits for the feedback thread.
        let sender = self.lock().take();
        drop(sender);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Arc<Sender>>> {
        self.sender.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long sent frames stay available for resending: twice the playout
/// delay, and at least a second.
fn resend_history(playout_delay: Duration) -> Duration {
    playout_delay.saturating_mul(2).max(Duration::from_secs(1))
}

/// The receiver's address with the port it answered. Keeps an IPv6 scope, so
/// a link-local receiver stays reachable.
fn udp_target(mut address: SocketAddr, port: u16) -> SocketAddr {
    address.set_port(port);
    address
}

/// `frame` behind a 7-byte ADTS header (AAC-LC, no CRC), or `None` for a
/// format AAC-LC cannot describe or a frame too long for the header.
fn adts(format: AudioFormat, frame: &[u8]) -> Option<Vec<u8>> {
    let [config_high, config_low] = audio_specific_config(format)?;
    let index = ((config_high & 0x07) << 1) | (config_low >> 7);
    let channels = (config_low >> 3) & 0x0f;
    let length = frame.len() + 7;
    if length >= 1 << 13 {
        return None;
    }
    let [length_high, length_low] = u16::try_from(length).ok()?.to_be_bytes();
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&[
        0xff,
        // MPEG-4, layer 0, no CRC.
        0xf1,
        // Profile AAC-LC (object type 2 minus one), rate index, channels' top bit.
        (1 << 6) | (index << 2) | (channels >> 2),
        ((channels & 0x03) << 6) | (length_high >> 3),
        (length_high << 5) | (length_low >> 3),
        // Frame length's low bits, then a buffer fullness of 0x7ff (variable rate).
        ((length_low & 0x07) << 5) | 0x1f,
        0xfc,
    ]);
    out.extend_from_slice(frame);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adts_headers_describe_aac_lc_frames() {
        let stereo = AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        assert_eq!(
            adts(stereo, &[1, 2, 3]).unwrap(),
            [0xff, 0xf1, 0x4c, 0x80, 0x01, 0x5f, 0xfc, 1, 2, 3]
        );
        let mono = AudioFormat {
            sample_rate: 44_100,
            channels: 1,
        };
        let long = adts(mono, &[0; 1000]).unwrap();
        // 44.1 kHz is index 4; 1007 bytes = 0b11_1110_1111.
        assert_eq!(&long[..7], &[0xff, 0xf1, 0x50, 0x40, 0x7d, 0xff, 0xfc]);
        assert!(adts(stereo, &vec![0; 8192]).is_none());
    }

    #[test]
    fn resend_history_covers_the_playout_delay_without_overflow() {
        assert_eq!(resend_history(Duration::from_millis(100)), Duration::from_secs(1));
        assert_eq!(resend_history(Duration::from_secs(3)), Duration::from_secs(6));
        assert_eq!(resend_history(Duration::MAX), Duration::MAX);
    }

    #[test]
    fn the_udp_target_keeps_an_ipv6_scope() {
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new("fe80::1".parse().unwrap(), 8009, 0, 3));
        let SocketAddr::V6(target) = udp_target(scoped, 47439) else {
            panic!("IPv6 stays IPv6");
        };
        assert_eq!((target.port(), target.scope_id()), (47439, 3));
        assert_eq!(
            udp_target("192.0.2.1:8009".parse().unwrap(), 5000).to_string(),
            "192.0.2.1:5000"
        );
    }
}
