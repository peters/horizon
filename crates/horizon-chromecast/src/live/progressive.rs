//! Progressive fragmented-MP4 stream: one endless HTTP response per receiver
//! connection, one fragment per access unit. Receivers play it with a small
//! start buffer that `LiveCast` then trims by briefly speeding up playback.
use super::{hls::nal_units, mp4};
use horizon_media::h264::{NAL_AUD, NAL_PPS, NAL_SPS, nal_type};
use std::{
    io::{self, Write},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::Duration,
};

/// Frames a connection may fall behind before it is dropped (~4 s at 30 fps).
const SUBSCRIBER_BACKLOG: usize = 120;
/// A GOP longer than this many frames is not kept for late joiners: they
/// wait for the next keyframe instead.
const MAX_GOP_FRAMES: usize = 600;
const RECV_POLL: Duration = Duration::from_millis(500);
/// Used for the last frame before a pause, when no later timestamp exists yet.
const FALLBACK_DURATION: u32 = mp4::TIMESCALE / 30;
/// No connection has started yet.
const NO_BASE: u64 = u64::MAX;

pub(crate) struct Sample {
    /// AVCC: 4-byte big-endian NAL lengths, without SPS/PPS/AUD.
    data: Vec<u8>,
    /// Presentation time in 90 kHz ticks from the first pushed unit.
    pts: u64,
    keyframe: bool,
}

#[derive(Default)]
struct State {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    init: Option<Arc<[u8]>>,
    gop: Vec<Arc<Sample>>,
    subscribers: Vec<SyncSender<Arc<Sample>>>,
    origin: Option<Duration>,
    last_pts: Option<u64>,
    last_keyframe: Option<u64>,
}

pub(crate) struct Stream {
    keyframe_interval: Duration,
    state: Mutex<State>,
    /// First pts written to the newest connection; its media time 0.
    playback_base: AtomicU64,
}

/// What a new connection needs: the init segment, the current GOP and a feed.
pub(crate) struct Subscription {
    init: Arc<[u8]>,
    backlog: Vec<Arc<Sample>>,
    feed: Receiver<Arc<Sample>>,
}

impl Stream {
    pub(crate) fn new(keyframe_interval: Duration) -> Self {
        Self {
            keyframe_interval,
            state: Mutex::new(State::default()),
            playback_base: AtomicU64::new(NO_BASE),
        }
    }

    /// Adds one Annex B access unit. Units before the first keyframe with
    /// parameter sets, and units whose timestamp goes backwards, are dropped.
    pub(crate) fn push(&self, annexb: &[u8], pts: Duration, keyframe: bool) {
        let mut state = self.lock();
        let origin = *state.origin.get_or_insert(pts);
        let pts = ticks(pts.saturating_sub(origin));
        if state.last_pts.is_some_and(|last| pts <= last) {
            return;
        }
        let mut data = Vec::with_capacity(annexb.len() + 16);
        for nal in nal_units(annexb) {
            match nal_type(nal) {
                Some(NAL_SPS) => state.sps = Some(nal.to_vec()),
                Some(NAL_PPS) => state.pps = Some(nal.to_vec()),
                Some(NAL_AUD) | None => {}
                Some(_) => {
                    data.extend_from_slice(&u32::try_from(nal.len()).unwrap_or(u32::MAX).to_be_bytes());
                    data.extend_from_slice(nal);
                }
            }
        }
        if keyframe
            && state.init.is_none()
            && let (Some(sps), Some(pps)) = (&state.sps, &state.pps)
        {
            state.init = mp4::init_segment(sps, pps).map(Into::into);
        }
        if state.init.is_none() || data.is_empty() || (state.gop.is_empty() && !keyframe) {
            return;
        }
        state.last_pts = Some(pts);
        let sample = Arc::new(Sample { data, pts, keyframe });
        if keyframe {
            state.gop.clear();
            state.last_keyframe = Some(pts);
        }
        // An incomplete GOP would hand late joiners frames that depend on
        // ones never sent, so past the cap there is no backlog until the next
        // keyframe. Existing connections keep receiving every frame.
        if keyframe || (!state.gop.is_empty() && state.gop.len() < MAX_GOP_FRAMES) {
            state.gop.push(sample.clone());
        } else {
            state.gop.clear();
        }
        // A connection that cannot keep up is dropped rather than buffered.
        state.subscribers.retain(|subscriber| {
            !matches!(
                subscriber.try_send(sample.clone()),
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
            )
        });
    }

    /// True when a keyframe is due so a connection can start soon.
    pub(crate) fn wants_keyframe(&self, pts: Duration) -> bool {
        let state = self.lock();
        let (Some(origin), Some(last)) = (state.origin, state.last_keyframe) else {
            return true;
        };
        ticks(pts.saturating_sub(origin)).saturating_sub(last) >= ticks(self.keyframe_interval)
    }

    /// A connection can start: an init segment and a keyframe exist.
    pub(crate) fn ready(&self) -> bool {
        let state = self.lock();
        state.init.is_some() && !state.gop.is_empty()
    }

    /// `None` until a connection can start from a keyframe.
    pub(crate) fn subscribe(&self) -> Option<Subscription> {
        let mut state = self.lock();
        let init = state.init.clone()?;
        if state.gop.is_empty() {
            return None;
        }
        let (sender, feed) = mpsc::sync_channel(SUBSCRIBER_BACKLOG);
        state.subscribers.push(sender);
        Some(Subscription {
            init,
            backlog: state.gop.clone(),
            feed,
        })
    }

    /// Ends every connection; their writers return once the feed closes.
    pub(crate) fn close(&self) {
        self.lock().subscribers.clear();
    }

    /// Seconds between the newest pushed frame and the newest connection's
    /// media time zero, or `None` before any connection started.
    pub(crate) fn newest_media_time(&self) -> Option<f64> {
        let base = self.playback_base.load(Ordering::Acquire);
        let last = self.lock().last_pts?;
        (base != NO_BASE).then(|| seconds(last.saturating_sub(base)))
    }

    /// Writes the init segment and fragments to `out` until the stream ends
    /// or the receiver disconnects. Timestamps restart at 0 for this connection.
    pub(crate) fn write_to(&self, out: &mut impl Write, subscription: Subscription) -> io::Result<()> {
        out.write_all(&subscription.init)?;
        let mut pending: Option<Arc<Sample>> = None;
        let mut base = None;
        let mut sequence = 1u32;
        let mut backlog = subscription.backlog.into_iter();
        loop {
            let next = match backlog.next() {
                Some(sample) => Some(sample),
                None => match subscription.feed.recv_timeout(RECV_POLL) {
                    Ok(sample) => Some(sample),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return out.flush(),
                },
            };
            let Some(next) = next else {
                continue;
            };
            // The feed may repeat the newest backlog frame.
            if pending.as_ref().is_some_and(|p| next.pts <= p.pts) {
                continue;
            }
            let start = *base.get_or_insert_with(|| {
                // Writers can start out of order; only a newer base may win.
                let _ = self
                    .playback_base
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                        (current == NO_BASE || next.pts > current).then_some(next.pts)
                    });
                next.pts
            });
            // Hold one frame so its duration is known exactly.
            if let Some(sample) = pending.replace(next.clone()) {
                let duration = u32::try_from(next.pts - sample.pts).unwrap_or(FALLBACK_DURATION);
                out.write_all(&mp4::fragment(
                    sequence,
                    sample.pts - start,
                    duration,
                    &sample.data,
                    sample.keyframe,
                ))?;
                out.flush()?;
                sequence = sequence.wrapping_add(1);
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn ticks(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos() * u128::from(mp4::TIMESCALE) / 1_000_000_000).unwrap_or(u64::MAX)
}

#[allow(clippy::cast_precision_loss)]
fn seconds(ticks: u64) -> f64 {
    ticks as f64 / f64::from(mp4::TIMESCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: [u8; 24] = [
        0x67, 0x64, 0x00, 0x1f, 0xac, 0xb2, 0x00, 0xa0, 0x0b, 0x76, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
        0x00, 0x07, 0x81, 0xe3, 0x06, 0x49,
    ];
    const PPS: [u8; 6] = [0x68, 0xeb, 0xc3, 0xcb, 0x22, 0xc0];

    fn unit(keyframe: bool) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, 0x09, 0xf0];
        if keyframe {
            for set in [&SPS[..], &PPS[..]] {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(set);
            }
        }
        out.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }, 0x88, 0x84]);
        out
    }

    fn frame(index: u32) -> Duration {
        Duration::from_secs(1) / 30 * index
    }

    #[test]
    fn waits_for_parameter_sets_and_a_keyframe() {
        let stream = Stream::new(Duration::from_millis(500));
        stream.push(&unit(false), frame(0), false);
        assert!(!stream.ready());
        assert!(stream.subscribe().is_none());
        stream.push(&unit(true), frame(1), true);
        assert!(stream.ready());
        assert!(!stream.wants_keyframe(frame(2)));
        assert!(stream.wants_keyframe(frame(16)));
    }

    #[test]
    fn late_joiners_start_at_the_last_keyframe_with_rebased_time() {
        let stream = Stream::new(Duration::from_millis(500));
        for index in 0..20 {
            stream.push(&unit(index % 15 == 0), frame(index), index % 15 == 0);
        }
        let subscription = stream.subscribe().unwrap();
        assert_eq!(subscription.backlog.len(), 5);
        assert!(subscription.backlog[0].keyframe);
        // Close the feed so the writer drains the backlog and returns.
        stream.lock().subscribers.clear();
        let mut out = Vec::new();
        stream.write_to(&mut out, subscription).unwrap();
        assert!(out.starts_with(&[0, 0, 0, 0x1c]) || out[4..8] == *b"ftyp");
        let first_tfdt = out.windows(4).position(|w| w == b"tfdt").unwrap() + 8;
        assert_eq!(
            u64::from_be_bytes(out[first_tfdt..first_tfdt + 8].try_into().unwrap()),
            0
        );
        assert_eq!(
            out.windows(4).filter(|w| *w == b"moof").count(),
            4,
            "the newest frame waits for its duration"
        );
        assert!((stream.newest_media_time().unwrap() - 4.0 / 30.0).abs() < 0.001);
    }

    #[test]
    fn slow_connections_are_dropped_and_timestamps_must_advance() {
        let stream = Stream::new(Duration::from_millis(500));
        stream.push(&unit(true), frame(0), true);
        let _idle = stream.subscribe().unwrap();
        for index in 1..=u32::try_from(SUBSCRIBER_BACKLOG).unwrap() + 1 {
            stream.push(&unit(false), frame(index), false);
        }
        assert!(stream.lock().subscribers.is_empty());
        let before = stream.lock().gop.len();
        stream.push(&unit(false), frame(3), false);
        assert_eq!(stream.lock().gop.len(), before);
    }

    #[test]
    fn a_gop_past_the_cap_has_no_late_join_backlog_until_the_next_keyframe() {
        let stream = Stream::new(Duration::from_secs(60));
        stream.push(&unit(true), frame(0), true);
        let existing = stream.subscribe().unwrap();
        let cap = u32::try_from(MAX_GOP_FRAMES).unwrap();
        for index in 1..=cap {
            stream.push(&unit(false), frame(index), false);
            // Keep the existing connection's feed drained.
            while existing.feed.try_recv().is_ok() {}
        }
        assert!(!stream.ready());
        assert!(stream.subscribe().is_none(), "no broken prefix for late joiners");
        assert_eq!(
            stream.lock().subscribers.len(),
            1,
            "existing connections keep their feed"
        );
        stream.push(&unit(true), frame(cap + 1), true);
        assert!(stream.ready());
        assert!(stream.subscribe().unwrap().backlog[0].keyframe);
    }

    #[test]
    fn the_playback_base_only_moves_forward() {
        let stream = Stream::new(Duration::from_millis(500));
        for index in 0..20 {
            stream.push(&unit(index % 15 == 0), frame(index), index % 15 == 0);
        }
        let newer = stream.subscribe().unwrap();
        stream.lock().subscribers.clear();
        stream.write_to(&mut Vec::new(), newer).unwrap();
        let base = stream.playback_base.load(Ordering::Acquire);
        assert_ne!(base, NO_BASE);
        // A slower writer that starts from an older frame must not move it back.
        let older = Subscription {
            init: stream.lock().init.clone().unwrap(),
            backlog: vec![Arc::new(Sample {
                data: vec![0, 0, 0, 1, 0x65],
                pts: 0,
                keyframe: true,
            })],
            feed: mpsc::sync_channel(1).1,
        };
        stream.write_to(&mut Vec::new(), older).unwrap();
        assert_eq!(stream.playback_base.load(Ordering::Acquire), base);
    }
}
