//! Fragmented MP4 (ISO BMFF) writer for one H.264 track: an initialisation
//! segment and one `moof`/`mdat` fragment per access unit.

/// Media timescale: the 90 kHz clock MPEG transports also use.
pub(crate) const TIMESCALE: u32 = 90_000;
const TRACK_ID: u32 = 1;
const SAMPLE_SYNC: u32 = 0x0200_0000;
const SAMPLE_NON_SYNC: u32 = 0x0101_0000;
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// `ftyp` + `moov` describing one AVC track built from `sps` and `pps`.
/// `None` when the SPS has no usable dimensions, or a parameter set does not
/// fit the 16-bit length that `avcC` stores.
pub(crate) fn init_segment(sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    if u16::try_from(sps.len()).is_err() || u16::try_from(pps.len()).is_err() {
        return None;
    }
    let (width, height) = sps_dimensions(sps)?;
    let (width, height) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
    let mut out = Vec::with_capacity(768);
    boxed(&mut out, *b"ftyp", |b| {
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&512u32.to_be_bytes());
        for brand in [b"isom", b"iso6", b"avc1", b"mp41"] {
            b.extend_from_slice(brand);
        }
    });
    boxed(&mut out, *b"moov", |moov| {
        full_box(moov, *b"mvhd", 0, 0, |b| {
            put_u32s(b, &[0, 0, 1000, 0, 0x0001_0000]);
            b.extend_from_slice(&0x0100u16.to_be_bytes());
            b.extend_from_slice(&[0; 10]);
            put_u32s(b, &UNITY_MATRIX);
            b.extend_from_slice(&[0; 24]);
            put_u32s(b, &[TRACK_ID + 1]);
        });
        boxed(moov, *b"trak", |trak| {
            full_box(trak, *b"tkhd", 0, 3, |b| {
                put_u32s(b, &[0, 0, TRACK_ID, 0, 0, 0, 0]);
                b.extend_from_slice(&[0; 8]);
                put_u32s(b, &UNITY_MATRIX);
                put_u32s(b, &[u32::from(width) << 16, u32::from(height) << 16]);
            });
            boxed(trak, *b"mdia", |mdia| {
                full_box(mdia, *b"mdhd", 0, 0, |b| {
                    put_u32s(b, &[0, 0, TIMESCALE, 0]);
                    // Language "und", packed ISO 639-2.
                    b.extend_from_slice(&[0x55, 0xc4, 0, 0]);
                });
                full_box(mdia, *b"hdlr", 0, 0, |b| {
                    put_u32s(b, &[0]);
                    b.extend_from_slice(b"vide");
                    b.extend_from_slice(&[0; 12]);
                    b.extend_from_slice(b"Video\0");
                });
                boxed(mdia, *b"minf", |minf| {
                    full_box(minf, *b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8]));
                    boxed(minf, *b"dinf", |dinf| {
                        full_box(dinf, *b"dref", 0, 0, |b| {
                            put_u32s(b, &[1]);
                            full_box(b, *b"url ", 0, 1, |_| {});
                        });
                    });
                    boxed(minf, *b"stbl", |stbl| {
                        full_box(stbl, *b"stsd", 0, 0, |b| {
                            put_u32s(b, &[1]);
                            avc1(b, width, height, sps, pps);
                        });
                        for kind in [*b"stts", *b"stsc", *b"stco"] {
                            full_box(stbl, kind, 0, 0, |b| put_u32s(b, &[0]));
                        }
                        full_box(stbl, *b"stsz", 0, 0, |b| put_u32s(b, &[0, 0]));
                    });
                });
            });
        });
        boxed(moov, *b"mvex", |mvex| {
            full_box(mvex, *b"trex", 0, 0, |b| put_u32s(b, &[TRACK_ID, 1, 0, 0, 0]));
        });
    });
    Some(out)
}

/// One fragment holding one sample: `sample` is AVCC (4-byte NAL lengths).
pub(crate) fn fragment(sequence: u32, decode_time: u64, duration: u32, sample: &[u8], keyframe: bool) -> Vec<u8> {
    let size = u32::try_from(sample.len()).unwrap_or(u32::MAX);
    let flags = if keyframe { SAMPLE_SYNC } else { SAMPLE_NON_SYNC };
    let mut out = Vec::with_capacity(sample.len() + 128);
    boxed(&mut out, *b"moof", |moof| {
        full_box(moof, *b"mfhd", 0, 0, |b| put_u32s(b, &[sequence]));
        boxed(moof, *b"traf", |traf| {
            // default-base-is-moof: data offsets count from this moof.
            full_box(traf, *b"tfhd", 0, 0x02_0000, |b| put_u32s(b, &[TRACK_ID]));
            full_box(traf, *b"tfdt", 1, 0, |b| {
                b.extend_from_slice(&decode_time.to_be_bytes());
            });
            // data offset, sample duration, size and flags present.
            full_box(traf, *b"trun", 0, 0x0701, |b| {
                put_u32s(b, &[1, 0, duration, size, flags]);
            });
        });
    });
    // The trun closes the moof; its data offset sits 16 bytes before the end
    // (followed by duration, size and flags) and points just past the mdat header.
    let moof_len = out.len();
    let offset = u32::try_from(moof_len + 8).unwrap_or(u32::MAX);
    out[moof_len - 16..moof_len - 12].copy_from_slice(&offset.to_be_bytes());
    out.extend_from_slice(&(size.saturating_add(8)).to_be_bytes());
    out.extend_from_slice(b"mdat");
    out.extend_from_slice(sample);
    out
}

fn avc1(out: &mut Vec<u8>, width: u16, height: u16, sps: &[u8], pps: &[u8]) {
    boxed(out, *b"avc1", |b| {
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        put_u32s(b, &[0x0048_0000, 0x0048_0000, 0]);
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0; 32]);
        b.extend_from_slice(&[0x00, 0x18, 0xff, 0xff]);
        boxed(b, *b"avcC", |c| {
            c.extend_from_slice(&[1, sps[1], sps[2], sps[3], 0xff, 0xe1]);
            put_sized(c, sps);
            c.push(1);
            put_sized(c, pps);
        });
    });
}

fn put_sized(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&u16::try_from(data.len()).unwrap_or(u16::MAX).to_be_bytes());
    out.extend_from_slice(data);
}

fn put_u32s(out: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

fn boxed(out: &mut Vec<u8>, kind: [u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&kind);
    body(out);
    let size = u32::try_from(out.len() - start).unwrap_or(u32::MAX);
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full_box(out: &mut Vec<u8>, kind: [u8; 4], version: u8, flags: u32, body: impl FnOnce(&mut Vec<u8>)) {
    boxed(out, kind, |b| {
        b.extend_from_slice(&(u32::from(version) << 24 | (flags & 0x00ff_ffff)).to_be_bytes());
        body(b);
    });
}

/// Display width and height from an SPS NAL unit (header byte included).
pub(crate) fn sps_dimensions(sps: &[u8]) -> Option<(u32, u32)> {
    if sps.len() < 4 {
        return None;
    }
    let rbsp = unescape(&sps[1..]);
    let mut bits = Bits::new(&rbsp);
    let profile = bits.read(8)?;
    bits.skip(16)?; // constraint flags + level
    bits.ue()?; // seq_parameter_set_id
    let mut chroma_format = 1;
    if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format = bits.ue()?;
        if chroma_format == 3 {
            bits.skip(1)?;
        }
        bits.ue()?; // bit_depth_luma_minus8
        bits.ue()?; // bit_depth_chroma_minus8
        bits.skip(1)?; // qpprime_y_zero_transform_bypass_flag
        if bits.read(1)? == 1 {
            let lists = if chroma_format == 3 { 12 } else { 8 };
            for list in 0..lists {
                if bits.read(1)? == 1 {
                    skip_scaling_list(&mut bits, if list < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    bits.ue()?; // log2_max_frame_num_minus4
    match bits.ue()? {
        0 => {
            bits.ue()?;
        }
        1 => {
            bits.skip(1)?;
            bits.se()?;
            bits.se()?;
            for _ in 0..bits.ue()? {
                bits.se()?;
            }
        }
        _ => {}
    }
    bits.ue()?; // max_num_ref_frames
    bits.skip(1)?; // gaps_in_frame_num_value_allowed_flag
    // Exp-Golomb fields can be near u32::MAX in malformed data: every step is checked.
    let width_mbs = bits.ue()?.checked_add(1)?;
    let height_map_units = bits.ue()?.checked_add(1)?;
    let frame_mbs_only = bits.read(1)?;
    if frame_mbs_only == 0 {
        bits.skip(1)?;
    }
    bits.skip(1)?; // direct_8x8_inference_flag
    let (mut crop_x, mut crop_y) = (0, 0);
    if bits.read(1)? == 1 {
        let (left, right, top, bottom) = (bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?);
        let (unit_x, unit_y) = match chroma_format {
            1 => (2, 2 * (2 - frame_mbs_only)),
            2 => (2, 2 - frame_mbs_only),
            // Monochrome (0) and 4:4:4 (3) crop in whole luma samples.
            _ => (1, 2 - frame_mbs_only),
        };
        crop_x = left.checked_add(right)?.checked_mul(unit_x)?;
        crop_y = top.checked_add(bottom)?.checked_mul(unit_y)?;
    }
    let width = width_mbs.checked_mul(16)?.checked_sub(crop_x)?;
    let height = height_map_units
        .checked_mul(16)?
        .checked_mul(2 - frame_mbs_only)?
        .checked_sub(crop_y)?;
    Some((width, height))
}

fn skip_scaling_list(bits: &mut Bits<'_>, size: usize) -> Option<()> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            next = (last + bits.se()? + 256) % 256;
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

/// Removes emulation-prevention bytes (`00 00 03` → `00 00`).
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &byte in data {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn read(&mut self, count: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self.data.get(self.position / 8)?;
            let bit = (byte >> (7 - self.position % 8)) & 1;
            value = value << 1 | u32::from(bit);
            self.position += 1;
        }
        Some(value)
    }

    fn skip(&mut self, count: u32) -> Option<()> {
        self.read(count).map(|_| ())
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.read(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.read(zeros)?)
    }

    fn se(&mut self) -> Option<i64> {
        let value = i64::from(self.ue()?);
        Some(if value % 2 == 1 { (value + 1) / 2 } else { -(value / 2) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPS and PPS from an x264 1280x720 High profile stream (720 = 736 - crop).
    const SPS_720P: [u8; 24] = [
        0x67, 0x64, 0x00, 0x1f, 0xac, 0xb2, 0x00, 0xa0, 0x0b, 0x76, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
        0x00, 0x07, 0x81, 0xe3, 0x06, 0x49,
    ];
    const PPS_720P: [u8; 6] = [0x68, 0xeb, 0xc3, 0xcb, 0x22, 0xc0];

    fn boxes(data: &[u8]) -> Vec<([u8; 4], usize)> {
        let mut found = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            found.push((data[at + 4..at + 8].try_into().unwrap(), size));
            assert!(size >= 8 && at + size <= data.len(), "box overruns its parent");
            at += size;
        }
        assert_eq!(at, data.len());
        found
    }

    #[test]
    fn parses_sps_dimensions_with_cropping() {
        assert_eq!(sps_dimensions(&SPS_720P), Some((1280, 720)));
        assert_eq!(sps_dimensions(&[0x67, 0x64]), None);
    }

    #[test]
    fn oversized_sps_dimensions_are_rejected_without_overflow() {
        // Baseline SPS whose pic_width_in_mbs_minus1 is 536_870_910: 16 times
        // the width in macroblocks does not fit in 32 bits.
        let sps = [0x67, 66, 0, 30, 0xdc, 0x00, 0x00, 0x00, 0x1f, 0xff, 0xff, 0xff, 0xe8];
        assert_eq!(sps_dimensions(&sps), None);
    }

    #[test]
    fn parameter_sets_too_long_for_avcc_are_rejected() {
        let mut sps = SPS_720P.to_vec();
        sps.resize(usize::from(u16::MAX) + 1, 0);
        assert!(init_segment(&sps, &PPS_720P).is_none());
        let mut pps = PPS_720P.to_vec();
        pps.resize(usize::from(u16::MAX) + 1, 0);
        assert!(init_segment(&SPS_720P, &pps).is_none());
    }

    #[test]
    fn init_segment_nests_cleanly() {
        let init = init_segment(&SPS_720P, &PPS_720P).unwrap();
        let top = boxes(&init);
        assert_eq!(
            top.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            [*b"ftyp", *b"moov"]
        );
        let moov = &init[top[0].1 + 8..];
        let children = boxes(moov);
        assert_eq!(
            children.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            [*b"mvhd", *b"trak", *b"mvex"]
        );
        assert!(init.windows(4).any(|w| w == b"avcC"));
    }

    #[test]
    fn fragment_data_offset_points_at_the_sample() {
        let sample = [0, 0, 0, 2, 0x65, 0x88];
        let frag = fragment(7, 90_000, 3000, &sample, true);
        let top = boxes(&frag);
        assert_eq!(
            top.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            [*b"moof", *b"mdat"]
        );
        let trun = frag.windows(4).position(|w| w == b"trun").unwrap() - 4;
        let offset = u32::from_be_bytes(frag[trun + 16..trun + 20].try_into().unwrap()) as usize;
        assert_eq!(&frag[offset..], &sample);
        let tfdt = frag.windows(4).position(|w| w == b"tfdt").unwrap() + 8;
        assert_eq!(u64::from_be_bytes(frag[tfdt..tfdt + 8].try_into().unwrap()), 90_000);
    }
}
