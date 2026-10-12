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
    #[cfg(unix)]
    if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Job {
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
        mut command: Command,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
    ) -> Result<(), String> {
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
                // A command that ended while something it started still holds its output.
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if matches!(child.try_wait(), Ok(Some(_))) {
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
        let status = child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
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
