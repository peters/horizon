use super::{RemotePanelStatusError as Error, protocol::RESPONSE_LIMIT};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    io::{self, Read, Write},
    os::fd::AsFd,
    process::{Child, ChildStdin, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(mut command: Command, input: &[u8], timeout: Duration) -> Result<Vec<u8>, Error> {
    let started = Instant::now();
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Error::ClientUnavailable
            } else {
                Error::QueryFailed
            }
        })?;
    let mut child = OwnedChild(child);
    let mut stdin = Some(child.0.stdin.take().ok_or(Error::QueryFailed)?);
    let mut stdout = child.0.stdout.take().ok_or(Error::QueryFailed)?;
    nonblocking(stdin.as_ref().ok_or(Error::QueryFailed)?)?;
    nonblocking(&stdout)?;
    let mut pending = input;
    let mut output = Vec::new();
    loop {
        if started.elapsed() >= timeout {
            return Err(Error::Deadline);
        }
        write_pending(&mut stdin, &mut pending)?;
        let finished = read_available(&mut stdout, &mut output)?;
        if let Some(status) = child.0.try_wait().map_err(|_| Error::QueryFailed)? {
            if !status.success() || !pending.is_empty() {
                return Err(Error::QueryFailed);
            }
            if finished {
                return Ok(output);
            }
        }
        thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
    }
}

fn nonblocking(fd: &impl AsFd) -> Result<(), Error> {
    let flags = fcntl_getfl(fd).map_err(|_| Error::QueryFailed)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|_| Error::QueryFailed)
}

fn write_pending(stdin: &mut Option<ChildStdin>, pending: &mut &[u8]) -> Result<(), Error> {
    if let Some(stream) = stdin {
        match stream.write(pending) {
            Ok(count) => *pending = &pending[count..],
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(_) => return Err(Error::QueryFailed),
        }
        if pending.is_empty() {
            stdin.take();
        }
    }
    Ok(())
}

fn read_available(stream: &mut impl Read, output: &mut Vec<u8>) -> Result<bool, Error> {
    let mut buffer = [0; 1024];
    // One read per pass keeps even a peer's endless output inside the elapsed-time budget.
    match stream.read(&mut buffer) {
        Ok(0) => Ok(true),
        Ok(count) if output.len() + count <= RESPONSE_LIMIT => {
            output.extend_from_slice(&buffer[..count]);
            Ok(false)
        }
        Ok(_) => Err(Error::InvalidResponse),
        Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(false),
        Err(_) => Err(Error::QueryFailed),
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
