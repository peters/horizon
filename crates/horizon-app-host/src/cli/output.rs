//! A blocked consumer can stall only a bounded detached sink, never the native controller.
use crate::lifecycle::{Operation as HostOperation, Reason as HostReason};
use crate::{Error, Result, runner::Control};
use std::{
    io::Write,
    sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError},
    time::{Duration, Instant},
};

struct Message {
    bytes: Vec<u8>,
    complete: SyncSender<std::io::Result<()>>,
}
pub(super) struct Output {
    send: SyncSender<Message>,
}
impl Output {
    pub(super) fn new(mut writer: impl Write + Send + 'static) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Message>(32);
        // The sink owns no actor, control, workspace or cleanup resources; never join a blocked writer.
        std::thread::Builder::new()
            .name("native-cli-output".into())
            .spawn(move || {
                for message in receive {
                    let written = writer.write_all(&message.bytes).and_then(|()| writer.flush());
                    let failed = written.is_err();
                    let _ = message.complete.send(written);
                    if failed {
                        return;
                    }
                }
            })
            .map_err(|error| Error::host_io(HostOperation::Output, &error))?;
        Ok(Self { send })
    }
    pub(super) fn send(&self, bytes: Vec<u8>, control: &Control) -> Result<()> {
        let end = Instant::now() + Duration::from_secs(2);
        let (complete, receive) = mpsc::sync_channel(1);
        let mut message = Message { bytes, complete };
        loop {
            control.remaining()?;
            if Instant::now() >= end {
                control.cancel();
                return Err(Error::host(HostOperation::Output, HostReason::OutputTimedOut));
            }
            match self.send.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Full(value)) => message = value,
                Err(TrySendError::Disconnected(_)) => {
                    control.cancel();
                    return Err(Error::host(HostOperation::Output, HostReason::ChannelClosed));
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        loop {
            control.remaining()?;
            if Instant::now() >= end {
                control.cancel();
                return Err(Error::host(HostOperation::Output, HostReason::OutputTimedOut));
            }
            match receive.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(error)) => {
                    control.cancel();
                    return Err(Error::host_io(HostOperation::Output, &error));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    control.cancel();
                    return Err(Error::host(HostOperation::Output, HostReason::ChannelClosed));
                }
                Err(RecvTimeoutError::Timeout) => (),
            }
        }
    }
    pub(super) fn terminal(&self, bytes: Vec<u8>) -> Result<()> {
        let (complete, receive) = mpsc::sync_channel(1);
        self.send.try_send(Message { bytes, complete }).map_err(|error| {
            Error::host(
                HostOperation::Output,
                match error {
                    TrySendError::Full(_) => HostReason::ChannelFull,
                    TrySendError::Disconnected(_) => HostReason::ChannelClosed,
                },
            )
        })?;
        match receive.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result.map_err(|error| Error::host_io(HostOperation::Output, &error)),
            Err(RecvTimeoutError::Timeout) => Err(Error::host(HostOperation::Output, HostReason::OutputTimedOut)),
            Err(RecvTimeoutError::Disconnected) => Err(Error::host(HostOperation::Output, HostReason::ChannelClosed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_failed_progress_write_cancels_without_another_event() {
        struct Failed;
        impl Write for Failed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = Output::new(Failed).unwrap();
        let control = Control::new(Duration::from_secs(10)).unwrap();
        assert_eq!(
            output.send(vec![0], &control),
            Err(Error::host_io(
                HostOperation::Output,
                &std::io::ErrorKind::BrokenPipe.into()
            ))
        );
        assert!(matches!(control.remaining(), Err(Error::Cancelled)));
    }

    struct Blocked(mpsc::Receiver<()>);
    impl Write for Blocked {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn first_stalled_write_cancels_without_filling_the_queue() {
        let (release, wait) = mpsc::channel();
        let output = Output::new(Blocked(wait)).unwrap();
        let control = Control::new(Duration::from_secs(10)).unwrap();
        let start = Instant::now();
        assert_eq!(
            output.send(vec![0], &control),
            Err(Error::host(HostOperation::Output, HostReason::OutputTimedOut))
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(matches!(control.remaining(), Err(Error::Cancelled)));
        drop(release);
    }
    #[test]
    fn stalled_write_obeys_external_cancellation_without_owning_control() {
        let (release, wait) = mpsc::channel();
        let output = Output::new(Blocked(wait)).unwrap();
        let control = std::sync::Arc::new(Control::new(Duration::from_secs(10)).unwrap());
        let retained = std::sync::Arc::clone(&control);
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            retained.cancel();
        });
        let start = Instant::now();
        assert!(matches!(output.send(vec![0], &control), Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(1));
        cancel.join().unwrap();
        drop(output);
        drop(release);
    }
}
