use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::{Error, Event, Request, Result};

/// Host-private guardian lease. Closing stdin is the cancellation and host-crash signal.
pub struct Process {
    child: Child,
    input: Option<ChildStdin>,
    events: mpsc::Receiver<Result<Event>>,
    complete: bool,
    started: bool,
}

impl Process {
    /// # Errors
    /// `worker` is a trusted bundled executable, never supplied by a project or MCP client.
    /// A journal failure occurs before the worker is allowed to start the declared command.
    pub fn start(worker: &Path, request: Request, journal: impl FnOnce(uuid::Uuid, u32) -> Result<()>) -> Result<Self> {
        let spec = request.spec;
        let mut command = Command::new(worker);
        command
            .arg("--guard")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|_| Error::StartFailed)?;
        let input = child.stdin.take().ok_or(Error::StartFailed)?;
        let output = child.stdout.take().ok_or(Error::StartFailed)?;
        let (send, events) = mpsc::channel();
        std::thread::Builder::new()
            .name("native-process-events".into())
            .spawn(move || {
                let mut output = BufReader::new(output);
                loop {
                    let mut line = Vec::new();
                    let count = output.by_ref().take(1025).read_until(b'\n', &mut line);
                    if count.is_err() || line.is_empty() {
                        let _ = send.send(Err(Error::CleanupUncertain));
                        break;
                    }
                    let value = if line.len() > 1024 || !line.ends_with(b"\n") {
                        Err(Error::CleanupUncertain)
                    } else {
                        serde_json::from_slice(&line).map_err(|_| Error::CleanupUncertain)
                    };
                    if send.send(value).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| {
                let _ = child.kill();
                let _ = child.wait();
                Error::StartFailed
            })?;
        let mut process = Self {
            child,
            input: Some(input),
            events,
            complete: false,
            started: false,
        };
        let input = process.input.as_mut().ok_or(Error::StartFailed)?;
        serde_json::to_writer(&mut *input, &spec).map_err(|_| Error::StartFailed)?;
        input
            .write_all(b"\n")
            .and_then(|()| input.flush())
            .map_err(|_| Error::StartFailed)?;
        match process.next(Duration::from_secs(5))? {
            Event::Armed {} => (),
            _ => return Err(Error::StartFailed),
        }
        journal(spec.operation, process.child.id())?;
        process.started = true; // a lost start write may still have reached the guardian
        let input = process.input.as_mut().ok_or(Error::StartFailed)?;
        input
            .write_all(b"start\n")
            .and_then(|()| input.flush())
            .map_err(|_| Error::StartFailed)?;
        Ok(process)
    }

    /// # Errors
    /// Only bounded typed events reach callers; process diagnostics stay private.
    pub fn next(&mut self, timeout: Duration) -> Result<Event> {
        if timeout.is_zero() || timeout > Duration::from_mins(30) {
            return Err(Error::Invalid);
        }
        let event = self.events.recv_timeout(timeout).map_err(|_| Error::Timeout)??;
        if matches!(event, Event::Complete { .. } | Event::Failed { .. }) {
            self.complete = true;
        }
        Ok(event)
    }

    /// # Errors
    /// Completion requires guardian acknowledgement and exit; lost cleanup is uncertainty.
    pub fn close(&mut self) -> Result<()> {
        self.input.take();
        if !self.started {
            // The handshake has not authorized any declared child execution.
            let _ = self.child.kill();
            self.child.wait().map_err(|_| Error::CleanupUncertain)?;
            self.complete = true;
            return Ok(());
        }
        if !self.complete {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err(Error::CleanupUncertain);
                }
                match self.next(remaining).map_err(|_| Error::CleanupUncertain)? {
                    Event::Complete { .. } => break,
                    Event::Failed { .. } => {
                        self.complete = true;
                        break;
                    }
                    _ => (),
                }
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if self.child.try_wait().map_err(|_| Error::CleanupUncertain)?.is_some() {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::CleanupUncertain);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.close();
        let _ = self.child.try_wait();
    }
}
