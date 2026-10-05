use crate::{Error, Result, crypto};
use horizon_media::h264::{NAL_IDR, NAL_PPS, NAL_SEI, NAL_SLICE, NAL_SPS, nal_type};
use ring::aead;

/// Packets contain one encoder access unit, never an arbitrary chunk of its byte stream.
pub(crate) struct VideoPackets {
    key: aead::LessSafeKey,
    nonce: u64,
    dimensions: (u16, u16),
}
impl VideoPackets {
    pub(crate) fn new(secret: &[u8], stream: u64, dimensions: (u16, u16)) -> Result<Self> {
        let key = crypto::derive(
            secret,
            &format!("DataStream-Salt{stream}"),
            "DataStream-Output-Encryption-Key",
        )?;
        Ok(Self {
            key: crypto::key(key.as_ref())?,
            nonce: 0,
            dimensions,
        })
    }
    pub(crate) fn configuration(&self, sps: &[u8], pps: &[u8]) -> Result<Vec<u8>> {
        if sps.len() < 4 || nal_type(sps) != Some(NAL_SPS) || nal_type(pps) != Some(NAL_PPS) {
            return Err(Error::Protocol("invalid H.264 parameter sets"));
        }
        let sps_len = u16::try_from(sps.len()).map_err(|_| Error::Protocol("SPS too large"))?;
        let pps_len = u16::try_from(pps.len()).map_err(|_| Error::Protocol("PPS too large"))?;
        let mut avcc = vec![1, sps[1], sps[2], sps[3], 255, 225];
        avcc.extend(sps_len.to_be_bytes());
        avcc.extend(sps);
        avcc.push(1);
        avcc.extend(pps_len.to_be_bytes());
        avcc.extend(pps);
        let mut packet = header(avcc.len(), 0, 0)?;
        packet[4] = 1;
        packet[6] = 0x16;
        packet[7] = 1;
        for (offset, value) in [
            (16, self.dimensions.0),
            (20, self.dimensions.1),
            (40, self.dimensions.0),
            (44, self.dimensions.1),
            (56, self.dimensions.0),
            (60, self.dimensions.1),
        ] {
            packet[offset..offset + 4].copy_from_slice(&f32::from(value).to_le_bytes());
        }
        packet.extend(avcc);
        Ok(packet)
    }
    pub(crate) fn frame(&mut self, nals: &[&[u8]], timestamp: u64, timeline: u64) -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        let mut keyframe = false;
        for nal in nals {
            let kind = nal_type(nal).ok_or(Error::Protocol("empty NAL"))?;
            if !matches!(kind, NAL_SLICE | NAL_IDR | NAL_SEI) {
                return Err(Error::Protocol("access unit must contain slices or SEI"));
            }
            keyframe |= kind == NAL_IDR;
            if nal.len() > 8 * 1024 * 1024 || payload.len() + nal.len() + 4 > 8 * 1024 * 1024 {
                return Err(Error::Protocol("access unit too large"));
            }
            payload.extend(
                u32::try_from(nal.len())
                    .map_err(|_| Error::Protocol("NAL too large"))?
                    .to_be_bytes(),
            );
            payload.extend(*nal);
        }
        if payload.is_empty() {
            return Err(Error::Protocol("empty access unit"));
        }
        let next = self
            .nonce
            .checked_add(1)
            .ok_or(Error::Protocol("video nonce exhausted"))?;
        let mut packet = header(payload.len() + 16, timestamp, timeline)?;
        if keyframe {
            packet[5] = 0x10;
        }
        let encrypted = crypto::seal(&self.key, self.nonce.to_le_bytes(), &packet, payload)?;
        self.nonce = next;
        packet.extend(encrypted);
        Ok(packet)
    }
}
fn header(length: usize, timestamp: u64, timeline: u64) -> Result<Vec<u8>> {
    let length = u32::try_from(length).map_err(|_| Error::Protocol("video packet too large"))?;
    let mut header = vec![0; 128];
    header[..4].copy_from_slice(&length.to_le_bytes());
    header[8..16].copy_from_slice(&timestamp.to_le_bytes());
    header[40..48].copy_from_slice(&timeline.to_le_bytes());
    Ok(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticates_header_and_payload_and_advances_nonce() {
        let secret = [7; 32];
        let mut sender = VideoPackets::new(&secret, 42, (1280, 720)).expect("key");
        let first = sender.frame(&[&[0x65, 1, 2]], 123, 456).expect("frame");
        let second = sender.frame(&[&[0x65, 1, 2]], 123, 456).expect("frame");
        assert_ne!(first, second);
        assert_eq!(first[5], 0x10);
        let key = crypto::key(
            crypto::derive(&secret, "DataStream-Salt42", "DataStream-Output-Encryption-Key")
                .expect("derive")
                .as_ref(),
        )
        .expect("key");
        assert_eq!(
            crypto::open(&key, 0u64.to_le_bytes(), &first[..128], first[128..].to_vec()).expect("decrypt"),
            [0, 0, 0, 3, 0x65, 1, 2]
        );
        let mut corrupt = first.clone();
        corrupt[40] ^= 1;
        assert!(crypto::open(&key, 0u64.to_le_bytes(), &corrupt[..128], corrupt[128..].to_vec()).is_err());
        assert!(sender.frame(&[&[0x67, 1]], 123, 456).is_err());
    }
}
