//! Feeds the shared encoder pipeline into a live cast.
use super::LiveCast;
use crate::Result;
use horizon_media::{encoder::AccessUnitSink, h264::AccessUnit};
use std::{sync::Arc, time::Duration};

/// An [`AccessUnitSink`] that pushes encoder output into a [`LiveCast`].
///
/// Pair it with `EncoderConfig::for_segments(options.segment)` so keyframes
/// arrive when the segmenter wants to cut. The cast stops when the last
/// `Arc` to it is dropped.
pub struct LiveCastSink {
    live: Arc<LiveCast>,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl LiveCastSink {
    #[must_use]
    pub fn new(live: Arc<LiveCast>) -> Self {
        Self {
            live,
            sps: Vec::new(),
            pps: Vec::new(),
        }
    }
}

impl AccessUnitSink for LiveCastSink {
    type Error = crate::Error;

    fn configure(&mut self, sps: &[u8], pps: &[u8]) -> Result<()> {
        sps.clone_into(&mut self.sps);
        pps.clone_into(&mut self.pps);
        Ok(())
    }

    fn send(&mut self, unit: &AccessUnit, pts: Duration) -> Result<()> {
        // Every segment must start decodable, so parameter sets lead each IDR.
        let annexb = unit.to_annexb(&[&self.sps, &self.pps]);
        self.live.push_annexb(&annexb, pts, unit.is_keyframe());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LiveOptions;
    use horizon_media::encoder::{EncoderConfig, INPUT_FRAME_RATE};

    #[test]
    fn encoder_output_fills_segments_that_start_with_parameter_sets() {
        for segment in [150, 500, 1_000, 1_480, 1_490, 2_000, 45_000, 60_000] {
            fill_segments(Duration::from_millis(segment));
        }
    }

    fn fill_segments(segment: Duration) {
        let options = LiveOptions {
            segment,
            ..LiveOptions::default()
        };
        let interval = EncoderConfig::for_segments(options.segment).keyframe_interval;
        // Nothing listens on the discard port: the control side fails, the stream still fills.
        let live = Arc::new(LiveCast::start("127.0.0.1:9".parse().unwrap(), options).unwrap());
        let mut sink = LiveCastSink::new(live.clone());
        sink.configure(&[0x67, 0x42], &[0x68, 0xce]).unwrap();
        let frame = Duration::from_secs(1) / INPUT_FRAME_RATE;
        for index in 0..(interval * 4) {
            let keyframe = index % interval == 0;
            let unit = AccessUnit {
                nals: vec![vec![if keyframe { 0x65 } else { 0x41 }, 1]],
            };
            sink.send(&unit, frame * index).unwrap();
        }
        let segmenter = super::super::lock(&live.segmenter);
        assert_eq!(segmenter.ready_segments(), 3, "{segment:?} segments");
        let segment = segmenter.segment(0).unwrap();
        let sps = segment.windows(5).position(|w| w == [0, 0, 1, 0x67, 0x42]);
        let idr = segment.windows(4).position(|w| w == [0, 0, 1, 0x65]);
        assert!(sps.is_some_and(|sps| idr.is_some_and(|idr| sps < idr)));
    }
}
