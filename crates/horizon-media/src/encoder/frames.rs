//! Capture frames handed to the encoder and the bounded input that repeats
//! the last safe image while capture is paused.
use std::{
    fmt,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

/// Why a capture frame was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    DimensionsOutOfRange,
    InvalidLength,
}

impl FrameError {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DimensionsOutOfRange => "source dimensions exceed capture limits",
            Self::InvalidLength => "invalid RGBA frame size",
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for FrameError {}

/// One RGBA capture image.
pub struct Frame {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
}
impl Frame {
    #[must_use]
    pub const fn supports_source_dimensions(width: usize, height: usize) -> bool {
        width > 0 && height > 0 && width <= 8192 && height <= 8192 && width * height <= 16 * 1024 * 1024
    }
    /// # Errors
    /// Returns an error for dimensions outside the capture limits or a buffer
    /// that is not exactly `width * height * 4` bytes.
    pub fn new(width: u16, height: u16, rgba: Vec<u8>) -> Result<Self, FrameError> {
        let pixels = usize::from(width) * usize::from(height);
        if !Self::supports_source_dimensions(usize::from(width), usize::from(height)) {
            return Err(FrameError::DimensionsOutOfRange);
        }
        if rgba.len() != pixels * 4 {
            return Err(FrameError::InvalidLength);
        }
        Ok(Self { width, height, rgba })
    }

    /// Writes the frame as raw RGBA, or as a PAM image when `source_frame` is set.
    /// # Errors
    /// Returns the writer's I/O error.
    pub fn write(&self, output: &mut impl Write, source_frame: bool) -> io::Result<()> {
        if source_frame {
            // PAM carries each crop's dimensions without a compressed-image round trip.
            write!(
                output,
                "P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",
                self.width, self.height
            )?;
        }
        output.write_all(&self.rgba)
    }
}

/// Frames from capture, with the last image repeated while capture is paused.
pub struct FrameInput {
    receiver: Receiver<Frame>,
    frozen: Arc<AtomicBool>,
    last: Option<Frame>,
}
impl FrameInput {
    #[must_use]
    pub fn new(receiver: Receiver<Frame>, frozen: Arc<AtomicBool>) -> Self {
        Self {
            receiver,
            frozen,
            last: None,
        }
    }
    /// The next frame to encode.
    /// # Errors
    /// Times out when capture produced nothing, or disconnects when it ended.
    pub fn next_frame(&mut self) -> Result<&Frame, RecvTimeoutError> {
        if self.frozen.load(Ordering::Relaxed) {
            // Repeat only the last validated source image, never the controls covering it.
            std::thread::sleep(Duration::from_millis(67));
            // The source channel has one slot; discarding once keeps cancellation bounded.
            let _ = self.receiver.try_recv();
        } else {
            self.last = Some(self.receiver.recv_timeout(Duration::from_millis(100))?);
        }
        self.last.as_ref().ok_or(RecvTimeoutError::Timeout)
    }
    /// True when unpaused capture has produced nothing for three seconds.
    pub fn stalled(&self, last_frame: &mut Instant, now: Instant) -> bool {
        if self.frozen.load(Ordering::Relaxed) {
            *last_frame = now;
            false
        } else {
            now.saturating_duration_since(*last_frame) > Duration::from_secs(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn frozen_capture_repeats_safe_frame_and_discards_queued_images() {
        let (send, receiver) = mpsc::sync_channel(1);
        let frozen = Arc::new(AtomicBool::new(false));
        let mut frames = FrameInput::new(receiver, frozen.clone());
        send.send(Frame::new(1, 1, vec![1; 4]).expect("valid frame"))
            .expect("first source image");
        assert_eq!(frames.next_frame().expect("source").rgba, [1; 4]);
        send.send(Frame::new(2, 1, vec![3; 8]).expect("valid frame"))
            .expect("queued source image");
        frozen.store(true, Ordering::Relaxed);
        assert_eq!(frames.next_frame().expect("freeze").rgba, [1; 4]);
        assert_eq!(frames.next_frame().expect("freeze again").width, 1);
        frozen.store(false, Ordering::Relaxed);
        assert!(matches!(frames.next_frame(), Err(RecvTimeoutError::Timeout)));
        send.send(Frame::new(2, 1, vec![5; 8]).expect("valid frame"))
            .expect("new source image");
        assert_eq!(frames.next_frame().expect("resume").rgba, [5; 8]);
    }

    #[test]
    fn paused_startup_waits_without_private_frames_then_resumes_with_a_fresh_image() {
        let (send, receiver) = mpsc::sync_channel(1);
        let frozen = Arc::new(AtomicBool::new(true));
        let mut frames = FrameInput::new(receiver, frozen.clone());
        send.send(Frame::new(1, 1, vec![3; 4]).expect("queued frame"))
            .expect("one slot");
        let started = Instant::now();
        let mut last_frame = started;
        for seconds in [4, 8, 30] {
            assert!(matches!(frames.next_frame(), Err(RecvTimeoutError::Timeout)));
            assert!(!frames.stalled(&mut last_frame, started + Duration::from_secs(seconds)));
            assert!(
                frames.last.is_none(),
                "paused startup cannot validate or emit a queued image"
            );
        }
        frozen.store(false, Ordering::Relaxed);
        assert!(matches!(frames.next_frame(), Err(RecvTimeoutError::Timeout)));
        assert!(!frames.stalled(&mut last_frame, started + Duration::from_secs(32)));
        assert!(
            frames.stalled(&mut last_frame, started + Duration::from_secs(34)),
            "unpaused starvation still fails"
        );
        send.send(Frame::new(1, 1, vec![7; 4]).expect("fresh frame"))
            .expect("resumed frame");
        assert_eq!(frames.next_frame().expect("resumed source").rgba, [7; 4]);
    }

    #[test]
    fn stop_interrupts_initial_paused_wait_without_a_frame() {
        let (_send, receiver) = mpsc::sync_channel(1);
        let frozen = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let cancellation = stop.clone();
        let (ready, entered) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut frames = FrameInput::new(receiver, frozen);
            assert!(matches!(frames.next_frame(), Err(RecvTimeoutError::Timeout)));
            ready.send(()).expect("wait ready");
            while !cancellation.load(Ordering::Relaxed) {
                assert!(matches!(frames.next_frame(), Err(RecvTimeoutError::Timeout)));
            }
            done.send(()).expect("stopped");
        });
        entered.recv_timeout(Duration::from_secs(1)).expect("worker ready");
        stop.store(true, Ordering::Relaxed);
        completed
            .recv_timeout(Duration::from_secs(1))
            .expect("paused polling remains stop responsive");
        worker.join().expect("wait worker joined");
    }

    #[test]
    fn source_frames_preserve_odd_dimensions_and_reject_unbounded_or_malformed_data() {
        let frame = Frame::new(3, 1, vec![9; 12]).expect("odd crop");
        let mut data = Vec::new();
        frame.write(&mut data, true).expect("PAM frame");
        assert!(data.starts_with(b"P7\nWIDTH 3\nHEIGHT 1\n"));
        assert!(data.ends_with(&[9; 12]));
        for (width, height) in [(0, 1), (8193, 1), (8192, 8192), (u16::MAX, u16::MAX)] {
            assert!(Frame::new(width, height, Vec::new()).is_err());
        }
        assert!(Frame::new(1, 1, vec![0; 3]).is_err());
    }
}
