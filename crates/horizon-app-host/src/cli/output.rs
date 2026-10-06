//! A blocked consumer can stall only a bounded detached sink, never the native controller.
use crate::{Error, Result, runner::Control};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

pub(super) struct Output {
    send: SyncSender<Vec<u8>>,
    failed: Arc<AtomicBool>,
    acknowledged: Arc<AtomicBool>,
}
impl Output {
    pub(super) fn new(mut writer: impl Write + Send + 'static) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Vec<u8>>(32);
        let failed = Arc::new(AtomicBool::new(false));
        let acknowledged = Arc::new(AtomicBool::new(false));
        let failed_sink = Arc::clone(&failed);
        let acknowledgement = Arc::clone(&acknowledged);
        // The sink owns no native actor, control, workspace or cleanup resources; never join a blocked writer.
        std::thread::Builder::new()
            .name("native-cli-output".into())
            .spawn(move || {
                for bytes in receive {
                    if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                        failed_sink.store(true, Ordering::Release);
                        return;
                    }
                    acknowledgement.store(true, Ordering::Release);
                }
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            send,
            failed,
            acknowledged,
        })
    }
    pub(super) fn send(&self, mut bytes: Vec<u8>, control: &Control) -> Result<()> {
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            control.remaining()?;
            if self.failed.load(Ordering::Acquire) || Instant::now() >= end {
                return Err(Error::Cancelled);
            }
            match self.send.try_send(bytes) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(value)) => bytes = value,
                Err(TrySendError::Disconnected(_)) => return Err(Error::Cancelled),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub(super) fn terminal(&self, bytes: Vec<u8>) -> Result<()> {
        self.send.try_send(bytes).map_err(|_| Error::Unavailable)?;
        let end = Instant::now() + Duration::from_secs(2);
        while !self.acknowledged.load(Ordering::Acquire) {
            if self.failed.load(Ordering::Acquire) || Instant::now() >= end {
                return Err(Error::Unavailable);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_sink_never_owns_controller_and_backpressure_obeys_cancellation() {
        struct Blocked(std::sync::mpsc::Receiver<()>);
        impl Write for Blocked {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (release, wait) = mpsc::channel();
        let output = Output::new(Blocked(wait)).unwrap();
        let control = Control::new(Duration::from_secs(10)).unwrap();
        for _ in 0..32 {
            output.send(vec![0], &control).unwrap();
        }
        control.cancel();
        let start = Instant::now();
        assert!(matches!(output.send(vec![0], &control), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_millis(100));
        drop(output);
        drop(release);
    }
}
