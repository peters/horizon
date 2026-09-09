//! Bounded local query-child I/O, independent of a worker protocol or admission.

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    io::{self, Read, Write},
    os::fd::AsFd,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum Error {
    #[error("worker query client is unavailable")]
    ClientUnavailable,
    #[error("worker query failed")]
    QueryFailed,
    #[error("worker query exceeded its deadline")]
    Deadline,
    #[error("worker query output exceeds its size limit")]
    OutputLimit,
}

pub(crate) fn run(
    command: Command,
    mut input: &[u8],
    timeout: Duration,
    response_limit: usize,
) -> Result<Vec<u8>, Error> {
    let expected = input.len() as u64;
    let result = exchange(command, &mut input, timeout, response_limit, || false, Some(expected))?;
    known_response(result, expected)
}

fn known_response(result: Exchange, expected: u64) -> Result<Vec<u8>, Error> {
    check_known(result.status, result.input.written(), expected)?;
    Ok(result.output)
}

fn check_known(status: ExitStatus, written: u64, expected: u64) -> Result<(), Error> {
    if status.success() && written == expected {
        Ok(())
    } else {
        Err(Error::QueryFailed)
    }
}

fn nonblocking(fd: &impl AsFd) -> Result<(), Error> {
    let flags = fcntl_getfl(fd).map_err(|_| Error::QueryFailed)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|_| Error::QueryFailed)
}

fn read_available(stream: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> Result<bool, Error> {
    let mut buffer = [0; 1024];
    // One read per pass keeps even a peer's endless output inside the elapsed-time budget.
    match stream.read(&mut buffer) {
        Ok(0) => Ok(true),
        Ok(count) if count <= limit.saturating_sub(output.len()) => {
            output.extend_from_slice(&buffer[..count]);
            Ok(false)
        }
        Ok(_) => Err(Error::OutputLimit),
        Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(false),
        Err(_) => Err(Error::QueryFailed),
    }
}

pub(crate) struct Exchange {
    pub status: ExitStatus,
    pub output: Vec<u8>,
    pub input: InputProgress,
}

/// Actual successful pipe writes; EOF is separate from a caller's known length.
pub(crate) enum InputProgress {
    Incomplete(u64),
    Complete(u64),
}

impl InputProgress {
    fn written(&self) -> u64 {
        match self {
            Self::Incomplete(written) | Self::Complete(written) => *written,
        }
    }
}

/// Stream bounded chunks and retain early/nonzero responses even when stdin closes.
/// Cancellation tears down only the owned local child; remote completion is unknown.
/// Blocking source reads, spawn and reap are outside the best-effort pipe deadline.
/// A known length retains legacy success-only early-exit/error precedence. Streaming
/// callers omit it so early/nonzero responses are drained even with incomplete input.
pub(crate) fn exchange(
    mut command: Command,
    input: &mut dyn Read,
    timeout: Duration,
    response_limit: usize,
    cancelled: impl Fn() -> bool,
    known_input: Option<u64>,
) -> Result<Exchange, Error> {
    if cancelled() {
        return Err(Error::QueryFailed);
    }
    let started = Instant::now();
    let admit = || {
        if cancelled() {
            Err(Error::QueryFailed)
        } else if started.elapsed() >= timeout {
            Err(Error::Deadline)
        } else {
            Ok(())
        }
    };
    let mut child = spawn(&mut command)?;
    let mut stdin = Some(child.0.stdin.take().ok_or(Error::QueryFailed)?);
    let mut stdout = child.0.stdout.take().ok_or(Error::QueryFailed)?;
    nonblocking(stdin.as_ref().ok_or(Error::QueryFailed)?)?;
    nonblocking(&stdout)?;
    let mut buffer = [0; 16 * 1024];
    let mut pending = 0..0;
    let mut input_complete = false;
    let mut written = 0;
    let mut output = Vec::new();
    loop {
        admit()?;
        let previous = (written, output.len());
        if let Some(expected) = known_input {
            if written != expected {
                stream_input(
                    &mut stdin,
                    input,
                    &mut buffer,
                    &mut pending,
                    &mut input_complete,
                    &mut written,
                    &admit,
                )?;
            }
            if written == expected {
                stdin.take();
            } else if stdin.is_none() {
                return Err(Error::QueryFailed);
            }
        }
        let finished = read_available(&mut stdout, &mut output, response_limit)?;
        if let Some(status) = child.0.try_wait().map_err(|_| Error::QueryFailed)? {
            if let Some(expected) = known_input {
                check_known(status, written, expected)?;
            }
            stdin.take();
            if finished {
                return Ok(Exchange {
                    status,
                    output,
                    input: if input_complete {
                        InputProgress::Complete(written)
                    } else {
                        InputProgress::Incomplete(written)
                    },
                });
            }
        }
        if known_input.is_none() {
            stream_input(
                &mut stdin,
                input,
                &mut buffer,
                &mut pending,
                &mut input_complete,
                &mut written,
                &admit,
            )?;
        }
        if previous == (written, output.len()) {
            thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
        }
    }
}

fn stream_input(
    stdin: &mut Option<ChildStdin>,
    input: &mut dyn Read,
    buffer: &mut [u8],
    pending: &mut std::ops::Range<usize>,
    complete: &mut bool,
    written: &mut u64,
    admit: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let Some(stream) = stdin.as_mut() else {
        return Ok(());
    };
    if pending.start == pending.end {
        let read = input.read(buffer);
        // A source read may block past the previous point-in-time admission.
        admit()?;
        match read {
            Ok(0) => {
                *complete = true;
                stdin.take();
                return Ok(());
            }
            Ok(count) => *pending = 0..count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(_) => return Err(Error::QueryFailed),
        }
    }
    admit()?;
    match stream.write(&buffer[pending.clone()]) {
        Ok(0) => {
            stdin.take();
        }
        Ok(count) => {
            pending.start += count;
            *written = written.checked_add(count as u64).ok_or(Error::QueryFailed)?;
        }
        Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
            stdin.take();
        }
        Err(_) => return Err(Error::QueryFailed),
    }
    Ok(())
}

struct OwnedChild(Child);

fn spawn(command: &mut Command) -> Result<OwnedChild, Error> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map(OwnedChild)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::ClientUnavailable
            } else {
                Error::QueryFailed
            }
        })
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[cfg(test)]
mod tests;
