//! Rolling live HLS window of MPEG-TS segments cut on keyframes.
use super::{h264, ts::TsMuxer};
use std::{collections::VecDeque, fmt::Write as _, sync::Arc, time::Duration};

const CLOCK_HZ: u128 = 90_000;

pub(crate) struct Segment {
    pub(crate) sequence: u64,
    pub(crate) duration: Duration,
    pub(crate) data: Arc<[u8]>,
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
        }
    }

    /// Adds one Annex B access unit. Frames before the first keyframe are
    /// dropped because the receiver cannot decode them.
    pub(crate) fn push(&mut self, annexb: &[u8], pts: Duration, keyframe: bool) {
        let origin = *self.origin.get_or_insert(pts);
        let pts = pts.saturating_sub(origin);
        match self.current_start {
            None if !keyframe => return,
            Some(start) if keyframe && self.due(pts.saturating_sub(start)) => self.finish(pts),
            _ => {}
        }
        if self.current_start.is_none() {
            self.current_start = Some(pts);
            self.muxer.write_tables(&mut self.current);
        }
        let clock = u64::try_from(pts.as_nanos() * CLOCK_HZ / 1_000_000_000).unwrap_or(u64::MAX);
        self.muxer
            .write_access_unit(&mut self.current, &h264::with_delimiter(annexb), clock, keyframe);
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
        let target = self
            .segments
            .iter()
            .map(|segment| segment.duration.as_secs_f64().ceil())
            .fold(self.target.as_secs_f64().ceil(), f64::max);
        let first = self
            .segments
            .front()
            .map_or(self.next_sequence, |segment| segment.sequence);
        let mut playlist = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{first}\n"
        );
        for segment in &self.segments {
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
        });
        self.next_sequence += 1;
        while self.segments.len() > self.window {
            self.segments.pop_front();
        }
    }
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
}
