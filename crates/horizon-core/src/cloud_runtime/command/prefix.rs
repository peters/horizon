//! Read a bounded binary prefix without retaining or logging the rest of an object.
use super::{Command, Error, Read, Result, Runner, Stdio, stop};
use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

impl Runner<'_> {
    pub(crate) fn prefix(&self, command: &mut Command, limit: usize, timeout: Duration) -> Result<Vec<u8>> {
        self.cancel.check()?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let result = (|| {
            let stdout = child.stdout.take().ok_or(Error::Invalid("Missing object output"))?;
            let (tx, rx) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let mut bytes = Vec::with_capacity(limit);
                let result = stdout.take(limit as u64).read_to_end(&mut bytes).map(|_| bytes);
                let _ = tx.send(result);
            });
            let start = Instant::now();
            loop {
                self.cancel.check()?;
                if start.elapsed() >= timeout {
                    return Err(Error::Invalid("Local object read timed out"));
                }
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(bytes) => return bytes.map_err(Error::from),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Invalid("Object reader stopped")),
                }
            }
        })();
        stop(&mut child);
        result
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn prefix_stops_large_output_and_observes_cancellation() {
        let cancel = horizon_cloud::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: vec![],
        };
        let bytes = runner
            .prefix(Command::new("sh").args(["-c", "yes x"]), 1025, Duration::from_secs(5))
            .unwrap();
        assert_eq!(bytes.len(), 1025);
        let signal = cancel.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            signal.cancel();
        });
        let start = Instant::now();
        assert!(
            runner
                .prefix(
                    Command::new("sh").args(["-c", "sleep 30"]),
                    1025,
                    Duration::from_secs(5)
                )
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        cancel_thread.join().unwrap();
    }
}
