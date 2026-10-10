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
pub struct Runner<'a> {
    pub cancel: &'a Cancellation,
    pub emit: &'a dyn Fn(Event),
    pub secrets: Vec<String>,
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
        self.capture(name, command, timeout, 4 * 1024 * 1024, false, |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok(self.redact(String::from_utf8_lossy(&output).into_owned()))
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
        command
            .stdin(std::fs::File::open(input)?)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
        let mut input = tempfile::tempfile()?;
        input.write_all(request)?;
        input.rewind()?;
        command.stdin(input).stdout(Stdio::piped()).stderr(Stdio::null());
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
        self.capture(name, command, timeout, limit as u64, true, |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        })?;
        Ok(output)
    }

    #[cfg(target_os = "linux")]
    pub(super) fn bounded_file(
        &self,
        command: &mut Command,
        output: &mut std::fs::File,
        limit: u64,
        timeout: Duration,
    ) -> Result<u64> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut length = 0;
        self.capture("Bounded source export", command, timeout, limit, false, |bytes| {
            output.write_all(bytes)?;
            length += bytes.len() as u64;
            Ok(())
        })?;
        Ok(length)
    }

    fn capture(
        &self,
        name: &'static str,
        command: &mut Command,
        timeout: Duration,
        limit: u64,
        log_stdout: bool,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
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
                                return Err(Error::Invalid("Command output exceeded its bound"));
                            }
                            if let Err(error) = consume(&bytes) {
                                stop(&mut child);
                                return Err(error);
                            }
                            output_length += bytes.len() as u64;
                            if !log_stdout {
                                continue;
                            }
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
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::Command(name))
                };
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
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn bounded_file_stops_a_live_producer_before_any_excess_bytes_are_written() {
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: vec![],
        };
        for oversized in [false, true] {
            let mut output = tempfile::tempfile().unwrap();
            let script = if oversized {
                "head -c 65536 /dev/zero; sleep 30"
            } else {
                "head -c 4096 /dev/zero"
            };
            let started = Instant::now();
            let result = runner.bounded_file(
                Command::new("sh").args(["-c", script]).stdin(Stdio::null()),
                &mut output,
                4096,
                Duration::from_secs(10),
            );
            assert_eq!(result.is_err(), oversized);
            assert!(output.metadata().unwrap().len() <= 4096);
            assert!(started.elapsed() < Duration::from_secs(2));
        }
    }

    #[test]
    fn a_timed_out_docker_command_is_a_stuck_daemon_only_when_docker_does_not_answer() {
        use std::path::Path;
        let temp = tempfile::tempdir().unwrap();
        let (cancel, lines) = (Cancellation::default(), std::cell::RefCell::new(Vec::new()));
        let emit = |event| {
            if let Event::Output(line) = event {
                lines.borrow_mut().push(line);
            }
        };
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        let create = |program: &Path| {
            let mut command = Command::new(program);
            command.args(["create", "--name", "horizon-contract-1"]);
            runner.run(
                "worker image contract creation",
                &mut command,
                Duration::from_millis(150),
            )
        };
        // Docker answers its health check: the timeout is a plain one.
        let docker = docker_daemon::fake_docker(
            temp.path(),
            "[ \"$1\" = version ] && echo 29.8.1 && exit\nexec sleep 30",
        );
        assert!(matches!(create(&docker), Err(Error::Invalid(TIMED_OUT))));
        assert!(lines.borrow().is_empty());
        // Docker answers nothing: the daemon is stuck.
        let docker = docker_daemon::fake_docker(temp.path(), "exec sleep 30");
        let result = create(&docker);
        assert!(
            matches!(
                result,
                Err(Error::DockerNotResponding("worker image contract creation"))
            ),
            "{result:?}"
        );
        assert_eq!(*lines.borrow(), ["Docker did not answer docker version within 5 s"]);
    }

    #[test]
    fn descendant_inheriting_output_cannot_bypass_deadline() {
        let cancel = Cancellation::default();
        let emit = |_| {};
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        let started = Instant::now();
        let result = runner.run(
            "test",
            Command::new("sh").args(["-c", "sleep 30 & exit 0"]),
            Duration::from_millis(150),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_interrupts_in_flight_image_commands_and_their_descendants() {
        for name in ["image build", "image push"] {
            let cancel = Cancellation::default();
            let descendant = std::cell::Cell::new(None::<u32>);
            let emit = |event| {
                if let Event::Output(line) = event
                    && let Some(pid) = line.trim().strip_prefix("operation-started ")
                {
                    descendant.set(Some(pid.parse().unwrap()));
                    cancel.cancel();
                }
            };
            let runner = Runner {
                cancel: &cancel,
                emit: &emit,
                secrets: Vec::new(),
            };
            let started = Instant::now();
            let result = runner.run(
                name,
                Command::new("sh").args(["-c", "sleep 30 & printf 'operation-started %s\\n' \"$!\"; wait"]),
                Duration::from_secs(10),
            );
            assert!(matches!(
                result,
                Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
            ));
            assert!(started.elapsed() < Duration::from_secs(2));
            let pid = descendant.get().expect("operation reported its live child");
            let deadline = Instant::now() + Duration::from_secs(2);
            let stopped = loop {
                let state = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .unwrap();
                let state = String::from_utf8(state.stdout).unwrap();
                if state.trim().is_empty() || state.trim_start().starts_with('Z') {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                thread::sleep(Duration::from_millis(10));
            };
            if !stopped {
                let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            }
            assert!(stopped, "cancellation left child {pid} running");
        }
    }
    #[test]
    fn stderr_does_not_corrupt_stdout_and_secrets_are_redacted() {
        let cancel = Cancellation::default();
        let emit = |_| {};
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec!["secret".into()],
        };
        let output = runner
            .run(
                "test",
                Command::new("sh").args(["-c", "printf secret; printf diagnostic >&2"]),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(output, "[REDACTED]");
    }

    #[test]
    fn parsed_stdout_stays_out_of_the_output_and_stderr_shows() {
        let events = std::cell::RefCell::new(Vec::new());
        let cancel = Cancellation::default();
        let emit = |event| events.borrow_mut().push(event);
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        let status = r#"echo '{"state":"ok","last_error":null}'"#;
        let output = runner
            .run_parsed("test", Command::new("sh").args(["-c", status]), Duration::from_secs(2))
            .unwrap();
        assert_eq!(output.trim(), r#"{"state":"ok","last_error":null}"#);
        let failing = format!("{status}; echo denied >&2; exit 1");
        let error = runner
            .run_parsed(
                "test",
                Command::new("sh").args(["-c", &failing]),
                Duration::from_secs(2),
            )
            .unwrap_err();
        assert!(matches!(error, Error::Command("test")));
        let lines: Vec<String> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Output(line) => Some(line.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, ["denied"], "only stderr is output");
    }

    #[test]
    fn private_authentication_input_cannot_reach_progress_output() {
        use std::os::unix::fs::PermissionsExt;
        let input = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(input.path(), "synthetic-private-credential").unwrap();
        std::fs::set_permissions(input.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let cancel = Cancellation::default();
        let emit = |event| events.borrow_mut().push(event);
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: vec![],
        };
        runner
            .private_input(
                Command::new("sh").args(["-c", "value=$(cat); printf '%s' \"$value\"; printf '%s' \"$value\" >&2"]),
                input.path(),
            )
            .unwrap();
        assert!(events.borrow().is_empty());
        std::fs::set_permissions(input.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            runner
                .private_input(Command::new("cat").arg("-"), input.path())
                .is_err()
        );
    }
}

#[cfg(all(test, unix))]
mod exchange_tests;
