//! A proof command's output: each line bounded, read through bounded queues, written to
//! the whole log up to its limit and passed to the dialog, and the command watched
//! until it ends, Stop is pressed or its time limit passes.
use super::{Event, Job, STEP_TIMEOUT};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The longest output line Horizon keeps; the rest of a longer line is dropped.
const LINE_LIMIT: usize = 16 * 1024;

/// Output lines waiting between a command's pipes and the proof.
const LINE_QUEUE: usize = 256;

/// The most a proof writes to its whole log.
const LOG_FILE_LIMIT: u64 = 64 * 1024 * 1024;

/// Reads one line of untrusted output into `line`, keeping at most [`LINE_LIMIT`]
/// bytes of it, and returns how many bytes it consumed.
fn read_bounded_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut consumed = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(consumed);
        }
        let (chunk, done) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(end) => (&buffer[..=end], true),
            None => (buffer, false),
        };
        let room = LINE_LIMIT.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let used = chunk.len();
        reader.consume(used);
        consumed += used;
        if done {
            return Ok(consumed);
        }
    }
}

/// Ends `child` and every process in its group, then reaps it.
fn end_tree(child: &mut std::process::Child) {
    end_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// Kills every process in the group the command `leader` started.
fn end_group(leader: u32) {
    #[cfg(unix)]
    if let Some(group) = i32::try_from(leader).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    #[cfg(not(unix))]
    let _ = leader;
}

/// The most output Horizon keeps from a command whose output it reads, such as a plan.
const CAPTURE_LIMIT: u64 = 4 * 1024 * 1024;

/// How long the last lines of an ended command may take to arrive while something it
/// started still holds its output open.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

impl Job {
    /// Records the lines still arriving from an ended command until its output closes
    /// or `grace` passes.
    fn drain(&self, received: &mpsc::Receiver<String>, log: &mut Option<std::fs::File>, grace: Duration) {
        let deadline = Instant::now() + grace;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match received.recv_timeout(left) {
                Ok(line) => self.record(log, line),
                Err(_) => return,
            }
        }
    }

    /// Runs `command` and returns what it wrote to standard output, at most
    /// [`CAPTURE_LIMIT`], within `limit`. Stop and the limit end its whole group.
    pub(super) fn capture(&self, mut command: Command, limit: Duration) -> Result<Vec<u8>, String> {
        use std::io::Read as _;
        // Docker commands built elsewhere, as for the image checks, go to the run's daemon too.
        if command.get_program() == "docker" {
            self.pin(&mut command);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", command.get_program().to_string_lossy()))?;
        // A reader that passes the limit says so, and the command ends at once instead
        // of waiting, blocked, on a full pipe until its time runs out.
        let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            let overflow = overflow.clone();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(pipe) = pipe {
                    let _ = pipe.take(CAPTURE_LIMIT + 1).read_to_end(&mut bytes);
                }
                if bytes.len() as u64 > CAPTURE_LIMIT {
                    overflow.store(true, std::sync::atomic::Ordering::Relaxed);
                    bytes.truncate(usize::try_from(CAPTURE_LIMIT).unwrap_or(usize::MAX));
                }
                bytes
            })
        };
        let stdout = read(
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        );
        let stderr = read(
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        );
        let started = Instant::now();
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            if self.cancel.is_cancelled() {
                end_tree(&mut child);
                self.remove_container();
                return Err("stopped".to_owned());
            }
            if overflow.load(std::sync::atomic::Ordering::Relaxed) {
                end_tree(&mut child);
                self.remove_container();
                return Err(format!("wrote more than {CAPTURE_LIMIT} bytes; stopped"));
            }
            if started.elapsed() > limit {
                end_tree(&mut child);
                self.remove_container();
                return Err(format!("still running after {} minutes; stopped", limit.as_secs() / 60));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        end_group(child.id());
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        if status.success() {
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&stderr);
            let tail: String = stderr
                .trim()
                .chars()
                .rev()
                .take(400)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(format!("exit {}: {tail}", status.code().unwrap_or(-1)))
        }
    }

    /// Writes one output line to the whole log, up to [`LOG_FILE_LIMIT`], and passes it
    /// to the dialog, which shows the last lines only, so a full queue drops one.
    pub(super) fn record(&self, log: &mut Option<std::fs::File>, line: String) {
        use std::io::Write;
        if let Some(file) = log.as_mut() {
            let logged = self.logged.get();
            if logged < LOG_FILE_LIMIT {
                let _ = writeln!(file, "{line}");
                self.logged.set(logged + line.len() as u64 + 1);
                if self.logged.get() >= LOG_FILE_LIMIT {
                    let _ = writeln!(file, "[Horizon stopped writing this log at {LOG_FILE_LIMIT} bytes]");
                }
            }
        }
        let _ = self.sender.try_send(Event::Line(line));
        self.ctx.request_repaint();
    }

    /// Runs `command`, sending each output line to the dialog and the log file: a
    /// preparation step (the bundle, the clone, a host restore) within
    /// [`STEP_TIMEOUT`], so a daemon or a feed that never answers cannot hold the proof.
    pub(super) fn stream(&self, command: Command, log: &mut Option<std::fs::File>) -> Result<(), String> {
        self.stream_within(command, log, Some(STEP_TIMEOUT))
    }

    /// As [`Self::stream`], stopping the command once it has run for `limit`.
    pub(super) fn stream_within(
        &self,
        command: Command,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
    ) -> Result<(), String> {
        self.stream_inner(command, log, limit, false).map(|_| ())
    }

    /// As [`Self::stream_within`] for a command that writes a report on standard output:
    /// its standard error is the log, and its standard output, at most [`CAPTURE_LIMIT`]
    /// bytes, is returned once it succeeded.
    pub(super) fn stream_reporting(
        &self,
        command: Command,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
    ) -> Result<Vec<u8>, String> {
        self.stream_inner(command, log, limit, true)
    }

    fn stream_inner(
        &self,
        mut command: Command,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
        report: bool,
    ) -> Result<Vec<u8>, String> {
        if command.get_program() == "docker" {
            self.pin(&mut command);
        }
        let started = Instant::now();
        // Its own process group, so Stop and the time limit end everything it started,
        // such as the build processes of a host restore, not only the command itself.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", command.get_program().to_string_lossy()))?;
        // Bounded, so output faster than Horizon writes it waits in the pipe instead.
        let (lines, received) = mpsc::sync_channel::<String>(LINE_QUEUE);
        // A report is collected whole, one byte past its limit at most, never logged.
        let collected = report.then(|| child.stdout.take()).flatten().map(|pipe| {
            std::thread::spawn(move || {
                use std::io::Read as _;
                let mut bytes = Vec::new();
                let _ = pipe.take(CAPTURE_LIMIT + 1).read_to_end(&mut bytes);
                bytes
            })
        });
        for pipe in [
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let lines = lines.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut bytes = Vec::new();
                // Bytes that are not UTF-8 become replacement characters, not the end of the log.
                while read_bounded_line(&mut reader, &mut bytes).is_ok_and(|read| read > 0) {
                    let line = String::from_utf8_lossy(&bytes);
                    if lines.send(line.trim_end_matches(['\n', '\r']).to_owned()).is_err() {
                        break;
                    }
                    bytes.clear();
                }
            });
        }
        drop(lines);
        loop {
            match received.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => self.record(log, line),
                // The command ended: its last lines, such as a build's final error, are
                // still on their way; they are read before anything it left goes.
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        self.drain(&received, log, DRAIN_GRACE);
                        break;
                    }
                }
                // Output closed, but the command may still run: it stays watched, so
                // Stop and the time limit still end it.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !matches!(child.try_wait(), Ok(None)) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            if self.cancel.is_cancelled() {
                end_tree(&mut child);
                self.remove_container();
                return Err("stopped".to_owned());
            }
            if limit.is_some_and(|limit| started.elapsed() > limit) {
                end_tree(&mut child);
                // Killing the docker client leaves its container running.
                self.remove_container();
                return Err(format!(
                    "still running after {} minutes; stopped",
                    started.elapsed().as_secs() / 60
                ));
            }
        }
        // The command ended: whatever it left running in its group goes too, so a step
        // never passes while something it started still runs.
        end_group(child.id());
        let status = child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            let report = collected
                .map(|reader| reader.join().unwrap_or_default())
                .unwrap_or_default();
            if report.len() as u64 > CAPTURE_LIMIT {
                return Err(format!("its report is larger than {CAPTURE_LIMIT} bytes"));
            }
            Ok(report)
        } else {
            Err(format!(
                "exit {} after {} s",
                status.code().unwrap_or(-1),
                started.elapsed().as_secs()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endless_output_line_is_kept_short() {
        let mut input = std::io::Cursor::new([vec![b'x'; LINE_LIMIT * 3], b"\nnext\n".to_vec()].concat());
        let mut line = Vec::new();
        assert_eq!(read_bounded_line(&mut input, &mut line).unwrap(), LINE_LIMIT * 3 + 1);
        assert_eq!(line.len(), LINE_LIMIT, "the rest of the line is dropped");
        line.clear();
        read_bounded_line(&mut input, &mut line).unwrap();
        assert_eq!(line, b"next\n", "the next line reads whole");
    }
}
