//! Rolling live HLS window of MPEG-TS segments cut on keyframes.
use super::{h264, ts::TsMuxer};
use std::{borrow::Cow, collections::VecDeque, fmt::Write as _, sync::Arc, time::Duration};

const CLOCK_HZ: u128 = 90_000;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
const START_CODE: [u8; 4] = [0, 0, 0, 1];
/// An open segment larger than this is dropped instead of growing without bound.
const MAX_OPEN_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct Segment {
    pub(crate) sequence: u64,
    pub(crate) duration: Duration,
    pub(crate) data: Arc<[u8]>,
    /// Follows a dropped segment: timestamps and continuity counters jump.
    pub(crate) discontinuity: bool,
}

pub(crate) struct Segmenter {
    target: Duration,
    window: usize,
    muxer: TsMuxer,
    current: Vec<u8>,
    current_start: Option<Duration>,
    next_sequence: u64,
    segments: VecDeque<Segment>,
    origin: Option<Duration>,
    /// Latest accepted timestamp; PCR may never move backwards.
    last_pts: Option<Duration>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    /// The next published segment follows a dropped one.
    gap: bool,
    /// Discontinuities that have left the window (`EXT-X-DISCONTINUITY-SEQUENCE`).
    discontinuity_sequence: u64,
}

impl Segmenter {
    pub(crate) fn new(target: Duration, window: usize) -> Self {
        Self {
            target,
            window: window.max(2),
            muxer: TsMuxer::default(),
            current: Vec::new(),
            current_start: None,
            next_sequence: 0,
            segments: VecDeque::new(),
            origin: None,
            last_pts: None,
            sps: None,
            pps: None,
            gap: false,
            discontinuity_sequence: 0,
        }
    }

    /// Adds one Annex B access unit. Frames before the first keyframe are
    /// dropped because the receiver cannot decode them.
    pub(crate) fn push(&mut self, annexb: &[u8], pts: Duration, keyframe: bool) {
        let origin = *self.origin.get_or_insert(pts);
        let pts = pts.saturating_sub(origin);
        // Without separate decode timestamps, reordered (B-frame) input would
        // make PCR regress; such units are dropped.
        if self.last_pts.is_some_and(|last| pts < last) {
            tracing::debug!("dropping an access unit whose timestamp went backwards");
            return;
        }
        self.last_pts = Some(pts);
        let carries_both = self.remember_parameter_sets(annexb);
        if let Some(start) = self.current_start
            && self.over_budget(pts.saturating_sub(start))
        {
            // No keyframe arrived in time: drop the open segment rather than
            // advertise one longer than the target or grow without bound.
            tracing::debug!("dropping an open HLS segment that outgrew its budget");
            self.current.clear();
            self.current_start = None;
            self.gap = self.next_sequence > 0;
        }
        match self.current_start {
            None if !keyframe => return,
            Some(start) if keyframe && self.due(pts.saturating_sub(start)) => self.finish(pts),
            _ => {}
        }
        let starting = self.current_start.is_none();
        if starting {
            self.current_start = Some(pts);
            self.muxer.write_tables(&mut self.current);
        }
        // PAT/PMT do not configure the decoder; each segment must carry SPS and PPS.
        let unit = if starting && !carries_both {
            Cow::Owned(self.with_parameter_sets(annexb))
        } else {
            Cow::Borrowed(annexb)
        };
        let clock = u64::try_from(pts.as_nanos() * CLOCK_HZ / 1_000_000_000).unwrap_or(u64::MAX);
        self.muxer
            .write_access_unit(&mut self.current, &h264::with_delimiter(&unit), clock, keyframe);
    }

    /// Caches the latest SPS/PPS; returns whether `annexb` carries both.
    fn remember_parameter_sets(&mut self, annexb: &[u8]) -> bool {
        let (mut sps, mut pps) = (false, false);
        for nal in nal_units(annexb) {
            match nal.first().map(|header| header & 0x1f) {
                Some(NAL_SPS) => {
                    sps = true;
                    self.sps = Some(nal.to_vec());
                }
                Some(NAL_PPS) => {
                    pps = true;
                    self.pps = Some(nal.to_vec());
                }
                _ => {}
            }
        }
        sps && pps
    }

    /// Inserts the cached SPS and PPS, after a leading delimiter if there is one.
    fn with_parameter_sets(&self, annexb: &[u8]) -> Vec<u8> {
        let delimiter = h264::leading_delimiter_len(annexb);
        let mut out = Vec::with_capacity(annexb.len() + 64);
        out.extend_from_slice(&annexb[..delimiter]);
        for set in [&self.sps, &self.pps].into_iter().flatten() {
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(set);
        }
        out.extend_from_slice(&annexb[delimiter..]);
        out
    }

    /// The constant `#EXT-X-TARGETDURATION`. Receivers hold back about three
    /// targets, so it is not padded: 0.5 s segments advertise 1.
    fn advertised_target(&self) -> u64 {
        let rounded = (self.target.as_millis() + 500) / 1000;
        u64::try_from(rounded).unwrap_or(u64::MAX).max(1)
    }

    /// An open segment may not round above the advertised target or exceed
    /// the byte budget.
    fn over_budget(&self, elapsed: Duration) -> bool {
        // EXTINF is written with three decimals, so 1.4995 s already prints as
        // 1.500 and rounds above a target of 1.
        let limit = Duration::from_secs(self.advertised_target()) + Duration::from_micros(499_500);
        elapsed >= limit || self.current.len() > MAX_OPEN_BYTES
    }

    /// True once the open segment has reached its target length, so the next
    /// frame should be a keyframe.
    pub(crate) fn wants_keyframe(&self, pts: Duration) -> bool {
        let (Some(origin), Some(start)) = (self.origin, self.current_start) else {
            return true;
        };
        self.due(pts.saturating_sub(origin).saturating_sub(start))
    }

    /// Frame durations rarely divide the target exactly (1/30 s), so allow a
    /// tenth of the target as slack instead of waiting for the next keyframe.
    fn due(&self, elapsed: Duration) -> bool {
        elapsed + self.target / 10 >= self.target
    }

    pub(crate) fn ready_segments(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn segment(&self, sequence: u64) -> Option<Arc<[u8]>> {
        self.segments
            .iter()
            .find(|segment| segment.sequence == sequence)
            .map(|segment| segment.data.clone())
    }

    pub(crate) fn playlist(&self) -> String {
        // HLS requires a constant target; every listed segment rounds to at most it.
        let target = self.advertised_target();
        let first = self
            .segments
            .front()
            .map_or(self.next_sequence, |segment| segment.sequence);
        let mut playlist = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{first}\n#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
            self.discontinuity_sequence
        );
        for segment in &self.segments {
            if segment.discontinuity {
                playlist.push_str("#EXT-X-DISCONTINUITY\n");
            }
            let _ = write!(
                playlist,
                "#EXTINF:{:.3},\nseg{}.ts\n",
                segment.duration.as_secs_f64(),
                segment.sequence
            );
        }
        playlist
    }

    fn finish(&mut self, end: Duration) {
        let Some(start) = self.current_start.take() else {
            return;
        };
        self.segments.push_back(Segment {
            sequence: self.next_sequence,
            duration: end.saturating_sub(start),
            data: std::mem::take(&mut self.current).into(),
            discontinuity: std::mem::take(&mut self.gap),
        });
        self.next_sequence += 1;
        while self.segments.len() > self.window {
            if self.segments.pop_front().is_some_and(|segment| segment.discontinuity) {
                self.discontinuity_sequence += 1;
            }
        }
    }
}

/// NAL units of an Annex B buffer, without start codes.
fn nal_units(annexb: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 <= annexb.len() {
        if annexb[at..at + 3] == [0, 0, 1] {
            starts.push(at + 3);
            at += 3;
        } else {
            at += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|&next| {
            let end = next - 3;
            // A four-byte start code leaves one zero before the three-byte one.
            if end > 0 && annexb[end - 1] == 0 { end - 1 } else { end }
        })
        .chain(std::iter::once(annexb.len()))
        .collect();
    starts
        .into_iter()
        .zip(ends)
        .map(move |(start, end)| &annexb[start..end.max(start)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDR: &[u8] = &[0, 0, 0, 1, 0x65, 1, 2, 3];
    const SLICE: &[u8] = &[0, 0, 0, 1, 0x41, 4, 5];

    fn feed(segmenter: &mut Segmenter, seconds: u64) {
        for frame in 0..seconds * 10 {
            let pts = Duration::from_millis(5_000 + frame * 100);
            let keyframe = frame % 10 == 0;
            segmenter.push(if keyframe { IDR } else { SLICE }, pts, keyframe);
        }
    }

    #[test]
    fn waits_for_a_keyframe() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 3);
        segmenter.push(SLICE, Duration::ZERO, false);
        assert!(segmenter.current.is_empty());
        assert!(segmenter.wants_keyframe(Duration::ZERO));
    }

    #[test]
    fn cuts_on_keyframes_and_keeps_a_bounded_window() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 3);
        feed(&mut segmenter, 6);
        assert_eq!(segmenter.ready_segments(), 3);
        let playlist = segmenter.playlist();
        assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:2\n"), "{playlist}");
        assert!(playlist.contains("#EXTINF:1.000,\nseg4.ts\n"), "{playlist}");
        assert!(!playlist.contains("seg5.ts"), "open segment is not listed");
        assert!(segmenter.segment(1).is_none());
        let data = segmenter.segment(4).unwrap();
        assert_eq!(data.len() % super::super::ts::PACKET_SIZE, 0);
        assert_eq!(data[0], 0x47);
        assert!(!segmenter.wants_keyframe(Duration::from_millis(10_500)));
        assert!(segmenter.wants_keyframe(Duration::from_secs(11)));
    }

    #[test]
    fn frame_rounding_does_not_double_segments() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 6);
        let frame = Duration::from_secs(1) / 30;
        for index in 0..95u32 {
            let keyframe = index % 30 == 0;
            segmenter.push(if keyframe { IDR } else { SLICE }, frame * index, keyframe);
        }
        assert_eq!(segmenter.ready_segments(), 3);
    }

    #[test]
    fn target_duration_rounds_instead_of_doubling() {
        let mut segmenter = Segmenter::new(Duration::from_millis(500), 6);
        let frame = Duration::from_secs(1) / 30;
        for index in 0..95u32 {
            let keyframe = index % 15 == 0;
            segmenter.push(if keyframe { IDR } else { SLICE }, frame * index, keyframe);
        }
        assert!(
            segmenter.playlist().contains("#EXT-X-TARGETDURATION:1\n"),
            "{}",
            segmenter.playlist()
        );
        assert!(segmenter.playlist().contains("#EXTINF:0.500,"));
    }

    #[test]
    fn segments_start_with_cached_parameter_sets() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 4);
        let first = [&[0, 0, 0, 1, 0x67, 9, 9][..], &[0, 0, 0, 1, 0x68, 8], IDR].concat();
        segmenter.push(&first, Duration::ZERO, true);
        for second in 1..=3 {
            segmenter.push(IDR, Duration::from_secs(second), true);
        }
        let later = segmenter.segment(2).unwrap();
        let sps = later.windows(5).position(|w| w == [0, 0, 1, 0x67, 9]);
        let idr = later.windows(4).position(|w| w == [0, 0, 1, 0x65]);
        assert!(
            sps.is_some_and(|sps| idr.is_some_and(|idr| sps < idr)),
            "segment 2 lacks SPS before IDR"
        );
    }

    #[test]
    fn a_late_keyframe_drops_the_open_segment_instead_of_growing_it() {
        let mut segmenter = Segmenter::new(Duration::from_millis(500), 6);
        segmenter.push(IDR, Duration::ZERO, true);
        for frame in 1..=20u64 {
            segmenter.push(SLICE, Duration::from_millis(frame * 100), false);
        }
        assert!(segmenter.current.is_empty(), "the 2 s open segment was dropped");
        assert!(segmenter.wants_keyframe(Duration::from_millis(2_000)));
        segmenter.push(IDR, Duration::from_millis(2_100), true);
        segmenter.push(IDR, Duration::from_millis(2_600), true);
        assert_eq!(segmenter.ready_segments(), 1);
        let playlist = segmenter.playlist();
        assert!(playlist.contains("#EXT-X-TARGETDURATION:1\n"), "{playlist}");
        assert!(playlist.contains("#EXTINF:0.500,"), "{playlist}");
    }

    #[test]
    fn target_duration_is_constant_and_unpadded() {
        for (segment, target) in [(500, 1), (1_000, 1), (1_400, 1), (2_000, 2)] {
            let segmenter = Segmenter::new(Duration::from_millis(segment), 4);
            assert_eq!(segmenter.advertised_target(), target);
        }
    }

    #[test]
    fn nal_units_split_three_and_four_byte_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3];
        let nals: Vec<&[u8]> = nal_units(&data).collect();
        assert_eq!(nals, [&[0x67, 1][..], &[0x68, 2], &[0x65, 3]]);
    }

    #[test]
    fn a_unit_repeating_only_the_sps_still_gets_the_cached_pps() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 4);
        let headers = [&[0, 0, 0, 1, 0x67, 9][..], &[0, 0, 0, 1, 0x68, 8], IDR].concat();
        segmenter.push(&headers, Duration::ZERO, true);
        let sps_only = [&[0, 0, 0, 1, 0x67, 9][..], IDR].concat();
        segmenter.push(&sps_only, Duration::from_secs(1), true);
        segmenter.push(IDR, Duration::from_secs(2), true);
        let second = segmenter.segment(1).unwrap();
        assert!(
            second.windows(5).any(|w| w == [0, 0, 1, 0x68, 8]),
            "segment 1 lacks the PPS"
        );
    }

    #[test]
    fn parameter_sets_follow_a_leading_delimiter() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 4);
        segmenter.push(&[0, 0, 0, 1, 0x67, 9, 0, 0, 0, 1, 0x68, 8], Duration::ZERO, false);
        let delimited = [&[0, 0, 0, 1, 0x09, 0xf0][..], IDR].concat();
        segmenter.push(&delimited, Duration::ZERO, true);
        let unit = segmenter.with_parameter_sets(&delimited);
        let types: Vec<u8> = nal_units(&unit).map(|nal| nal[0] & 0x1f).collect();
        assert_eq!(types, [9, 7, 8, 5]);
        let delimiters = segmenter
            .current
            .windows(5)
            .filter(|w| *w == [0, 0, 1, 0x09, 0xf0])
            .count();
        assert_eq!(delimiters, 1, "one access unit, one delimiter");
    }

    #[test]
    fn a_segment_that_would_print_as_one_and_a_half_seconds_is_dropped() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 4);
        let frame = Duration::from_secs(1) / 30;
        segmenter.push(IDR, Duration::ZERO, true);
        for index in 1..45 {
            segmenter.push(SLICE, frame * index, false);
        }
        segmenter.push(IDR, frame * 45, true);
        assert_eq!(segmenter.ready_segments(), 0, "{}", segmenter.playlist());
        assert!(!segmenter.playlist().contains("EXTINF:1.500"));
    }

    #[test]
    fn a_dropped_segment_marks_the_next_one_as_a_discontinuity() {
        let mut segmenter = Segmenter::new(Duration::from_millis(500), 2);
        segmenter.push(IDR, Duration::ZERO, true);
        segmenter.push(IDR, Duration::from_millis(500), true);
        for frame in 6..=20u64 {
            segmenter.push(SLICE, Duration::from_millis(frame * 100), false);
        }
        segmenter.push(IDR, Duration::from_millis(2_100), true);
        segmenter.push(IDR, Duration::from_millis(2_600), true);
        let playlist = segmenter.playlist();
        assert!(playlist.contains("#EXT-X-DISCONTINUITY-SEQUENCE:0\n"), "{playlist}");
        assert!(
            playlist.contains("#EXT-X-DISCONTINUITY\n#EXTINF:0.500,\nseg1.ts"),
            "{playlist}"
        );
        segmenter.push(IDR, Duration::from_millis(3_100), true);
        segmenter.push(IDR, Duration::from_millis(3_600), true);
        let playlist = segmenter.playlist();
        assert!(playlist.contains("#EXT-X-DISCONTINUITY-SEQUENCE:1\n"), "{playlist}");
        assert!(!playlist.contains("#EXT-X-DISCONTINUITY\n"), "{playlist}");
    }

    #[test]
    fn units_whose_timestamp_goes_backwards_are_dropped() {
        let mut segmenter = Segmenter::new(Duration::from_secs(1), 4);
        segmenter.push(IDR, Duration::ZERO, true);
        segmenter.push(SLICE, Duration::from_millis(120), false);
        let before = segmenter.current.len();
        segmenter.push(SLICE, Duration::from_millis(40), false);
        assert_eq!(segmenter.current.len(), before, "a reordered unit is not muxed");
        segmenter.push(SLICE, Duration::from_millis(160), false);
        assert!(segmenter.current.len() > before);
    }
}
