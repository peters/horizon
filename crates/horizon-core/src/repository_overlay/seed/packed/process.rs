use super::{SeedError, protocol::Header};
use git2::Oid;
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    io::{self, Read, Write},
    os::fd::AsFd,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) struct Session<'a> {
    child: Option<OwnedChild>,
    input: ChildStdin,
    output: ChildStdout,
    cancelled: Box<dyn Fn() -> bool + 'a>,
    timeout: Duration,
    deadline: Instant,
}

impl<'a> Session<'a> {
    pub(super) fn spawn(
        mut command: Command,
        timeout: Duration,
        cancelled: Box<dyn Fn() -> bool + 'a>,
    ) -> Result<Self, SeedError> {
        if cancelled() {
            return Err(SeedError::Cancelled);
        }
        let mut child = OwnedChild(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| SeedError::Source)?,
        );
        let input = child.0.stdin.take().ok_or(SeedError::Source)?;
        let output = child.0.stdout.take().ok_or(SeedError::Source)?;
        nonblocking(&input)?;
        nonblocking(&output)?;
        Ok(Self {
            child: Some(child),
            input,
            output,
            cancelled,
            timeout,
            deadline: Instant::now(),
        })
    }

    pub(super) fn info(&mut self, oid: Oid) -> Result<Header, SeedError> {
        self.deadline = Instant::now() + self.timeout;
        self.request("info", oid)
            .and_then(|()| self.header(oid))
            .map_err(|error| match error.kind() {
                io::ErrorKind::ConnectionAborted => SeedError::Cancelled,
                _ => SeedError::Source,
            })
    }

    pub(super) fn request(&mut self, command: &str, oid: Oid) -> io::Result<()> {
        let request = format!("{command} {oid}\n");
        let mut pending = request.as_bytes();
        while !pending.is_empty() {
            self.check()?;
            match self.input.write(pending) {
                Ok(0) => return Err(failed()),
                Ok(n) => pending = &pending[n..],
                Err(e) if retry(&e) => self.pause(),
                Err(_) => return Err(failed()),
            }
        }
        Ok(())
    }

    pub(super) fn header(&mut self, oid: Oid) -> io::Result<Header> {
        let mut line = [0; 80];
        for n in 0..line.len() {
            self.exact(&mut line[n..=n])?;
            if line[n] == b'\n' {
                return Header::parse(&line[..n], oid).ok_or_else(failed);
            }
        }
        Err(failed())
    }

    pub(super) fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.output.read(buffer) {
                Ok(n) => return Ok(n),
                Err(e) if retry(&e) => self.pause(),
                Err(_) => return Err(failed()),
            }
        }
    }

    pub(super) fn exact(&mut self, mut buffer: &mut [u8]) -> io::Result<()> {
        while !buffer.is_empty() {
            let n = self.read(buffer)?;
            if n == 0 {
                return Err(failed());
            }
            buffer = &mut buffer[n..];
        }
        Ok(())
    }

    fn check(&self) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "raw object source cancelled",
            ));
        }
        if self.child.is_none() {
            return Err(failed());
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "raw object source timed out"));
        }
        Ok(())
    }

    fn pause(&self) {
        thread::sleep(Duration::from_millis(2).min(self.deadline.saturating_duration_since(Instant::now())));
    }

    pub(super) fn poison(&mut self) {
        self.child.take();
    }

    #[cfg(test)]
    pub(super) fn id(&self) -> Option<u32> {
        self.child.as_ref().map(|child| child.0.id())
    }
}

pub(super) fn failed() -> io::Error {
    io::Error::other("raw object source failed")
}
fn retry(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock)
}
fn nonblocking(fd: &impl AsFd) -> Result<(), SeedError> {
    let flags = fcntl_getfl(fd).map_err(|_| SeedError::Source)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|_| SeedError::Source)
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
