use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::Duration,
};

pub(crate) struct FrameInput {
    receiver: Receiver<Vec<u8>>,
    frozen: Arc<AtomicBool>,
    last: Option<Vec<u8>>,
}
impl FrameInput {
    pub(crate) fn new(receiver: Receiver<Vec<u8>>, frozen: Arc<AtomicBool>) -> Self {
        Self {
            receiver,
            frozen,
            last: None,
        }
    }
    pub(crate) fn next(&mut self) -> Result<&[u8], RecvTimeoutError> {
        if self.frozen.load(Ordering::Relaxed) && self.last.is_some() {
            // Repeat only the last validated source image, never the controls covering it.
            std::thread::sleep(Duration::from_millis(67));
            while self.receiver.try_recv().is_ok() {}
        } else {
            self.last = Some(self.receiver.recv_timeout(Duration::from_millis(100))?);
        }
        self.last.as_deref().ok_or(RecvTimeoutError::Timeout)
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
        send.send(vec![1, 2]).expect("first source image");
        assert_eq!(frames.next().expect("source"), [1, 2]);
        send.send(vec![3, 4]).expect("queued source image");
        frozen.store(true, Ordering::Relaxed);
        assert_eq!(frames.next().expect("freeze"), [1, 2]);
        assert_eq!(frames.next().expect("freeze again"), [1, 2]);
        frozen.store(false, Ordering::Relaxed);
        assert!(matches!(frames.next(), Err(RecvTimeoutError::Timeout)));
        send.send(vec![5, 6]).expect("new source image");
        assert_eq!(frames.next().expect("resume"), [5, 6]);
    }
}
