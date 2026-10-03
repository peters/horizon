use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::Duration,
};

pub(crate) struct Frame {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
}
impl Frame {
    pub(crate) const fn supports_source_dimensions(width: usize, height: usize) -> bool {
        width > 0 && height > 0 && width <= 8192 && height <= 8192 && width * height <= 16 * 1024 * 1024
    }
    pub(crate) fn new(width: u16, height: u16, rgba: Vec<u8>) -> crate::Result<Self> {
        let pixels = usize::from(width) * usize::from(height);
        if !Self::supports_source_dimensions(usize::from(width), usize::from(height)) {
            return Err(crate::Error::Protocol("source dimensions exceed capture limits"));
        }
        if rgba.len() != pixels * 4 {
            return Err(crate::Error::Protocol("invalid RGBA frame size"));
        }
        Ok(Self { width, height, rgba })
    }

    pub(crate) fn write(&self, output: &mut impl Write, source_frame: bool) -> io::Result<()> {
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

pub(crate) struct FrameInput {
    receiver: Receiver<Frame>,
    frozen: Arc<AtomicBool>,
    last: Option<Frame>,
}
impl FrameInput {
    pub(crate) fn new(receiver: Receiver<Frame>, frozen: Arc<AtomicBool>) -> Self {
        Self {
            receiver,
            frozen,
            last: None,
        }
    }
    pub(crate) fn next(&mut self) -> Result<&Frame, RecvTimeoutError> {
        if self.frozen.load(Ordering::Relaxed) && self.last.is_some() {
            // Repeat only the last validated source image, never the controls covering it.
            std::thread::sleep(Duration::from_millis(67));
            while self.receiver.try_recv().is_ok() {}
        } else {
            self.last = Some(self.receiver.recv_timeout(Duration::from_millis(100))?);
        }
        self.last.as_ref().ok_or(RecvTimeoutError::Timeout)
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
        assert_eq!(frames.next().expect("source").rgba, [1; 4]);
        send.send(Frame::new(2, 1, vec![3; 8]).expect("valid frame"))
            .expect("queued source image");
        frozen.store(true, Ordering::Relaxed);
        assert_eq!(frames.next().expect("freeze").rgba, [1; 4]);
        assert_eq!(frames.next().expect("freeze again").width, 1);
        frozen.store(false, Ordering::Relaxed);
        assert!(matches!(frames.next(), Err(RecvTimeoutError::Timeout)));
        send.send(Frame::new(2, 1, vec![5; 8]).expect("valid frame"))
            .expect("new source image");
        assert_eq!(frames.next().expect("resume").rgba, [5; 8]);
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
