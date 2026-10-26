//! Bounded command output and cancellation for task-owned process groups.
mod build_progress;
mod prefix;
pub mod terminal_progress;
use super::{Error, Event, Result, docker_daemon};
use horizon_cloud::Cancellation;
use std::{
    io::{Read, Seek, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
/// The error of a local command that outlived its timeout.
pub(super) const TIMED_OUT: &str = "Local operation timed out";
/// The error of a command whose output passed its bound.
pub(super) const OUTPUT_BOUND: &str = "Command output exceeded its bound";
pub struct Runner<'a> {
    pub cancel: &'a Cancellation,
    pub emit: &'a dyn Fn(Event),
    pub secrets: Vec<String>,
}
/// The answer of a bounded command whose exit status is part of the answer: a non-zero
/// exit is a result, not a failure. Each stream is capped; output beyond its cap is an
/// error, so a caller never receives a silently cut answer.
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
}
impl Runner<'_> {
    /// # Errors
    /// Reports spawn failure, cancellation, timeout and unsuccessful exit.
    pub fn run(&self, name: &'static str, command: &mut Command, timeout: Duration) -> Result<String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if name == "image build" {
            let steps = std::cell::RefCell::new(build_progress::BuildSteps::default());
            let emit = |event| {
                if let Event::Output(line) = &event
                    && let Some(progress) = steps.borrow_mut().observe(line)
                {
                    (self.emit)(Event::Progress(progress));
                }
                (self.emit)(event);
            };
            return Runner {
                cancel: self.cancel,
                emit: &emit,
                secrets: self.secrets.clone(),
            }
            .spawn(name, command, timeout);
        }
        self.spawn(name, command, timeout)
    }
    /// Like [`Runner::run`] for a command whose stdout is only parsed: stdout stays out of
    /// the output, while stderr still shows to explain a failure.
    ///
    /// # Errors
    /// Reports spawn failure, cancellation, timeout and unsuccessful exit.
    pub fn run_parsed(&self, name: &'static str, command: &mut Command, timeout: Duration) -> Result<String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut output = Vec::new();
        self.capture_success(name, command, timeout, 4 * 1024 * 1024, false, |is_stdout, bytes| {
            if is_stdout {
                output.extend_from_slice(bytes);
            }
            Ok(())
        })?;
        Ok(self.redact(String::from_utf8_lossy(&output).into_owned()))
    }
    /// Like [`Runner::run`] for a command whose answer is its output and its exit status:
    /// a non-zero exit is part of the answer, not an error. Each stream is capped at `limit`
    /// and output beyond it is an error, as in [`Runner::run`].
    ///
    /// # Errors
    /// Reports spawn failure, cancellation, timeout, an unreadable stream and output that
    /// exceeded its bound.
    pub fn run_captured(
        &self,
        name: &'static str,
        command: &mut Command,
        limit: u64,
        timeout: Duration,
    ) -> Result<Captured> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        // Stdout is capped by `capture`; stderr takes the same bound here.
        let status = self.capture(name, command, timeout, limit, false, |is_stdout, bytes| {
            if !is_stdout {
                if bytes.len() as u64 > limit.saturating_sub(stderr.len() as u64) {
                    return Err(Error::Invalid(OUTPUT_BOUND));
                }
                stderr.extend_from_slice(bytes);
                return Ok(());
            }
            stdout.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok(Captured {
            stdout: self.redact(String::from_utf8_lossy(&stdout).into_owned()),
            stderr: self.redact(String::from_utf8_lossy(&stderr).into_owned()),
            exit_status: status.code(),
        })
    }
    /// # Errors
    /// Sends a caller-selected private file on stdin without exposing it in argv or output.
    pub fn private_input(&self, command: &mut Command, input: &std::path::Path) -> Result<()> {
        super::settings::validate_private_key_file(input)?;
        self.private_payload(command, input)
    }
    /// # Errors
    /// Uploads a bounded private structured payload without logging either stream.
    pub fn private_payload(&self, command: &mut Command, input: &std::path::Path) -> Result<()> {
        let meta = std::fs::metadata(input)?;
        if !meta.is_file() || meta.len() > 256 * 1024 {
            return Err(Error::Invalid("Invalid private runtime payload"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(Error::Invalid("Runtime payload must be private (0600)"));
            }
        }
        self.private_upload(command, std::fs::File::open(input)?.into())
    }
    /// # Errors
    /// Uploads a bounded private structured payload from memory, without logging either stream.
    pub fn private_bytes(&self, command: &mut Command, input: &[u8]) -> Result<()> {
        if input.len() > 256 * 1024 {
            return Err(Error::Invalid("Invalid private runtime payload"));
        }
        self.private_upload(command, private_memory(input)?.into())
    }
    fn private_upload(&self, command: &mut Command, input: Stdio) -> Result<()> {
        command.stdin(input).stdout(Stdio::null()).stderr(Stdio::null());
        Runner {
            cancel: self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        }
        .spawn("Private runtime upload", command, Duration::from_secs(30))?;
        Ok(())
    }
    /// Exchange a bounded structured request without logging either stream.
    /// The caller must anchor any mutation intent before invoking this method.
    ///
    /// # Errors
    /// Rejects oversized requests/replies, cancellation, timeout and unsuccessful exit.
    pub fn private_exchange(&self, command: &mut Command, request: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        const LIMIT: usize = 64 * 1024;
        if request.len() > LIMIT {
            return Err(Error::Invalid("Private request exceeded its bound"));
        }
        self.cancel.check()?;
        command
            .stdin(private_memory(request)?)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Runner {
            cancel: self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        }
        .spawn_bytes("Private worker request", command, timeout, LIMIT)
        .map_err(|error| match error {
            Error::Command(_) => Error::PrivateTransport,
            other => other,
        })
    }
    /// # Errors
    /// Streams an already verified private frame without logging source or replies.
    pub(crate) fn private_file_exchange(
        &self,
        command: &mut Command,
        mut input: std::fs::File,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        if input.metadata()?.len() > horizon_cloud_protocol::membership::Source::MAX_BYTES + 65540 {
            return Err(Error::PrivateTransport);
        }
        input.rewind()?;
        command.stdin(input).stdout(Stdio::piped()).stderr(Stdio::null());
        Runner {
            cancel: self.cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        }
        .spawn_bytes("Private source transfer", command, timeout, 64 * 1024)
        .map_err(|error| match error {
            Error::Command(_) => Error::PrivateTransport,
            other => other,
        })
    }
    /// # Errors
    /// Runs a bounded object pack without collecting binary output into memory.
    pub fn to_file(
        &self,
        name: &'static str,
        command: &mut Command,
        input: &std::path::Path,
        output: &std::path::Path,
        timeout: Duration,
    ) -> Result<()> {
        let output = std::fs::OpenOptions::new().write(true).create_new(true).open(output)?;
        command
            .stdin(std::fs::File::open(input)?)
            .stdout(output)
            .stderr(Stdio::piped());
        self.spawn(name, command, timeout)?;
        Ok(())
    }
    fn spawn(&self, name: &'static str, command: &mut Command, timeout: Duration) -> Result<String> {
        let bytes = self.spawn_bytes(name, command, timeout, 4 * 1024 * 1024)?;
        Ok(self.redact(String::from_utf8_lossy(&bytes).into_owned()))
    }
    fn spawn_bytes(
        &self,
        name: &'static str,
        command: &mut Command,
        timeout: Duration,
        limit: usize,
    ) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        self.capture_success(name, command, timeout, limit as u64, true, |is_stdout, bytes| {
            if is_stdout {
                output.extend_from_slice(bytes);
            }
            Ok(())
        })?;
        Ok(output)
    }

    pub(super) fn bounded_file(
        &self,
        command: &mut Command,
        output: &mut std::fs::File,
        limit: u64,
        timeout: Duration,
    ) -> Result<u64> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut length = 0;
        self.capture_success(
            "Bounded source export",
            command,
            timeout,
            limit,
            false,
            |is_stdout, bytes| {
                if is_stdout {
                    output.write_all(bytes)?;
                    length += bytes.len() as u64;
                }
                Ok(())
            },
        )?;
        Ok(length)
    }

    fn capture_success(
        &self,
        name: &'static str,
        command: &mut Command,
        timeout: Duration,
        limit: u64,
        log_stdout: bool,
        consume: impl FnMut(bool, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let status = self.capture(name, command, timeout, limit, log_stdout, consume)?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Command(name))
        }
    }

    /// Runs one command in its own process group, hands each stream to `consume` and
    /// returns how it exited, also when it failed.
    fn capture(
        &self,
        name: &'static str,
        command: &mut Command,
        timeout: Duration,
        limit: u64,
        log_stdout: bool,
        mut consume: impl FnMut(bool, &[u8]) -> Result<()>,
    ) -> Result<std::process::ExitStatus> {
        self.cancel.check()?;
        (self.emit)(Event::Progress(super::progress::Progress::activity(name)));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let rx = output_streams(&mut child);
        let started = Instant::now();
        let mut output_length = 0_u64;
        let mut status = None;
        let mut pending_stdout = Vec::new();
        let mut pending_stderr = Vec::new();
        loop {
            let mut disconnected = false;
            let mut received = false;
            for _ in 0..64 {
                match rx.try_recv() {
                    Ok(Ok(OutputChunk { stdout, bytes })) => {
                        received = true;
                        if stdout {
                            if bytes.len() as u64 > limit.saturating_sub(output_length) {
                                stop(&mut child);
                                return Err(Error::Invalid(OUTPUT_BOUND));
                            }
                            if let Err(error) = consume(true, &bytes) {
                                stop(&mut child);
                                return Err(error);
                            }
                            output_length += bytes.len() as u64;
                            if !log_stdout {
                                continue;
                            }
                        } else if let Err(error) = consume(false, &bytes) {
                            stop(&mut child);
                            return Err(error);
                        }
                        let pending = if stdout {
                            &mut pending_stdout
                        } else {
                            &mut pending_stderr
                        };
                        pending.extend_from_slice(&bytes);
                        while let Some(newline) = pending.iter().position(|b| *b == b'\n') {
                            let line = pending.drain(..=newline).collect::<Vec<_>>();
                            self.log(&line);
                        }
                        if pending.len() > 8192 {
                            self.log(pending);
                            pending.clear();
                        }
                    }
                    Ok(Err(error)) => {
                        stop(&mut child);
                        return Err(error.into());
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if self.cancel.is_cancelled() || started.elapsed() > timeout {
                stop(&mut child);
                self.cancel.check()?;
                return Err(self.timed_out(name, command)?);
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status
                && disconnected
            {
                self.log(&pending_stdout);
                self.log(&pending_stderr);
                return Ok(status);
            }
            if !received {
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
    /// A Docker command that outlived its time is a stuck daemon when Docker does not
    /// answer a health check either; any other timeout stays a plain one.
    /// # Errors
    /// Cancellation during the health check.
    fn timed_out(&self, name: &'static str, command: &Command) -> Result<Error> {
        let cancelled = || self.cancel.is_cancelled();
        let stuck = docker_daemon::Target::of(command).is_some_and(|target| {
            target.probe(docker_daemon::PROBE_TIMEOUT, &cancelled) == docker_daemon::Health::NotResponding
        });
        self.cancel.check()?;
        if !stuck {
            return Ok(Error::Invalid(TIMED_OUT));
        }
        (self.emit)(Event::Output(format!(
            "Docker did not answer docker version within {} s",
            docker_daemon::PROBE_TIMEOUT.as_secs()
        )));
        Ok(Error::DockerNotResponding(name))
    }
    fn redact(&self, mut value: String) -> String {
        for secret in &self.secrets {
            if !secret.is_empty() {
                value = value.replace(secret, "[REDACTED]");
            }
        }
        value
    }
    fn log(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            (self.emit)(Event::Output(
                self.redact(String::from_utf8_lossy(bytes).trim_end().to_owned()),
            ));
        }
    }
}
struct OutputChunk {
    stdout: bool,
    bytes: Vec<u8>,
}
fn output_streams(child: &mut Child) -> mpsc::Receiver<std::io::Result<OutputChunk>> {
    let (tx, rx) = mpsc::sync_channel(256);
    for (stdout, stream) in [
        (true, child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
        (false, child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
    ] {
        if let Some(mut stream) = stream {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut buffer = [0; 4096];
                loop {
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            let _ = tx.send(Err(error));
                            break;
                        }
                        Ok(n) => {
                            if tx
                                .send(Ok(OutputChunk {
                                    stdout,
                                    bytes: buffer[..n].to_vec(),
                                }))
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            });
        }
    }
    rx
}

fn stop(child: &mut Child) {
    #[cfg(unix)]
    if let Some(id) = rustix::process::Pid::from_raw(child.id().cast_signed()) {
        let _ = rustix::process::kill_process_group(id, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}
/// A file for a private stdin. On Linux it lives in memory only and no path ever names it, so
/// the bytes never reach a disk; elsewhere, or where the kernel refuses a memfd, it is a
/// temporary file that is unlinked as soon as it is created, so its random name exists only
/// for that moment. Unlike a pipe, it never blocks, also when a descendant of the command keeps
/// its stdin open.
fn private_memory(bytes: &[u8]) -> Result<std::fs::File> {
    #[cfg(target_os = "linux")]
    let mut file = rustix::fs::memfd_create("horizon-private", rustix::fs::MemfdFlags::CLOEXEC)
        .map(std::fs::File::from)
        .or_else(|_| tempfile::tempfile())?;
    #[cfg(not(target_os = "linux"))]
    let mut file = tempfile::tempfile()?;
    file.write_all(bytes)?;
    file.rewind()?;
    Ok(file)
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, unix))]
mod exchange_tests;
