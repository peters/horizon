//! Supervised run of the external authenticator command.
//!
//! The command gets one JSON object on stdin and prints one on stdout. A
//! worker thread owns the process, so the browser driver loop never waits
//! for it. Cancel, the deadline and drop all kill the process.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MAX_STDOUT_BYTES: u64 = 256 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long stdout may stay open after the process exited, for example
/// when a child process of the command inherited it.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
/// How long a drop waits for the supervisor to kill the process, so the
/// command does not outlive a driver thread that ends right after.
const DROP_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommandOutcome {
    /// The process exited; its bounded stdout.
    Exited(Vec<u8>),
    TimedOut,
    Failed(String),
}

#[derive(Debug)]
pub(crate) struct CommandRun {
    cancel: Arc<AtomicBool>,
    outcome: mpsc::Receiver<CommandOutcome>,
}

impl CommandRun {
    pub(crate) fn start(argv: &[String], input: Vec<u8>, timeout: Duration) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (outcome_tx, outcome) = mpsc::channel();
        let argv = argv.to_vec();
        let worker_cancel = Arc::clone(&cancel);
        let spawned = std::thread::Builder::new()
            .name("browser-webauthn-command".into())
            .spawn(move || {
                let _ = outcome_tx.send(supervise(&argv, input, timeout, &worker_cancel));
            });
        if let Err(error) = spawned {
            let (failed_tx, failed) = mpsc::channel();
            let _ = failed_tx.send(CommandOutcome::Failed(format!(
                "could not start a supervisor thread: {error}"
            )));
            return Self {
                cancel,
                outcome: failed,
            };
        }
        Self { cancel, outcome }
    }

    /// The outcome once the process finished; `None` while it runs.
    pub(crate) fn poll(&self) -> Option<CommandOutcome> {
        match self.outcome.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(CommandOutcome::Failed("the command supervisor stopped".to_string()))
            }
        }
    }
}

impl Drop for CommandRun {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.outcome.recv_timeout(DROP_GRACE);
    }
}

fn supervise(argv: &[String], input: Vec<u8>, timeout: Duration, cancel: &AtomicBool) -> CommandOutcome {
    let Some((program, arguments)) = argv.split_first() else {
        return CommandOutcome::Failed("the authenticator command is empty".to_string());
    };
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return CommandOutcome::Failed(format!("could not start {program}: {error}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A large request can fill the pipe before the command reads it.
        let _ = std::thread::Builder::new()
            .name("browser-webauthn-stdin".into())
            .spawn(move || {
                let _ = stdin.write_all(&input);
            });
    }
    let (stdout_tx, stdout_rx) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        let _ = std::thread::Builder::new()
            .name("browser-webauthn-stdout".into())
            .spawn(move || {
                // Read one byte past the limit and stop; dropping the pipe
                // ends the read side, so memory stays bounded. The thread
                // ends when the last writer closes stdout.
                let mut bytes = Vec::new();
                let read = stdout.take(MAX_STDOUT_BYTES + 1).read_to_end(&mut bytes);
                let _ = stdout_tx.send(read.map(|_| bytes));
            });
    }
    let deadline = Instant::now() + timeout;
    let mut output = None;
    loop {
        if output.is_none()
            && let Ok(read) = stdout_rx.try_recv()
        {
            if read.as_ref().is_ok_and(|bytes| bytes.len() as u64 > MAX_STDOUT_BYTES) {
                stop(&mut child);
                return CommandOutcome::Failed("the command printed too much output".to_string());
            }
            output = Some(read);
        }
        if cancel.load(Ordering::Acquire) {
            stop(&mut child);
            return CommandOutcome::Failed("cancelled".to_string());
        }
        if Instant::now() >= deadline {
            stop(&mut child);
            return CommandOutcome::TimedOut;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                stop(&mut child);
                return CommandOutcome::Failed(format!("could not observe the command: {error}"));
            }
        }
    }
    let output = match output {
        Some(read) => Ok(read),
        None => stdout_rx.recv_timeout(OUTPUT_GRACE),
    };
    match output {
        Ok(Ok(bytes)) if bytes.len() as u64 > MAX_STDOUT_BYTES => {
            CommandOutcome::Failed("the command printed too much output".to_string())
        }
        Ok(Ok(bytes)) => CommandOutcome::Exited(bytes),
        Ok(Err(error)) => CommandOutcome::Failed(format!("could not read the command output: {error}")),
        Err(_) => CommandOutcome::Failed("the command output stayed open after it exited".to_string()),
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    fn wait(run: &CommandRun, timeout: Duration) -> Option<CommandOutcome> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(outcome) = run.poll() {
                return Some(outcome);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn process_exists(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[test]
    fn the_command_reads_stdin_and_its_stdout_is_returned() {
        let run = CommandRun::start(&shell("cat"), b"{\"op\":\"get\"}".to_vec(), Duration::from_secs(5));
        assert_eq!(
            wait(&run, Duration::from_secs(5)),
            Some(CommandOutcome::Exited(b"{\"op\":\"get\"}".to_vec()))
        );
    }

    #[test]
    fn missing_programs_and_oversized_output_fail() {
        let missing = CommandRun::start(
            &["/nonexistent/authenticator".to_string()],
            Vec::new(),
            Duration::from_secs(5),
        );
        assert!(matches!(
            wait(&missing, Duration::from_secs(5)),
            Some(CommandOutcome::Failed(_))
        ));
        let noisy = CommandRun::start(&shell("head -c 400000 /dev/zero"), Vec::new(), Duration::from_secs(5));
        assert!(matches!(
            wait(&noisy, Duration::from_secs(5)),
            Some(CommandOutcome::Failed(_))
        ));
        let endless = CommandRun::start(&shell("exec cat /dev/zero"), Vec::new(), Duration::from_secs(30));
        assert!(
            matches!(wait(&endless, Duration::from_secs(5)), Some(CommandOutcome::Failed(_))),
            "a command that never stops writing fails at the output limit, not at the deadline"
        );
    }

    #[test]
    fn deadline_and_drop_kill_the_process() {
        let dir = tempfile::tempdir().expect("temp");
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
        let run = CommandRun::start(&shell(&script), Vec::new(), Duration::from_millis(300));
        assert_eq!(wait(&run, Duration::from_secs(5)), Some(CommandOutcome::TimedOut));
        let pid = std::fs::read_to_string(&pid_file).expect("pid");
        assert!(!process_exists(pid.trim()), "timed out command still runs");

        std::fs::remove_file(&pid_file).expect("reset");
        let run = CommandRun::start(&shell(&script), Vec::new(), Duration::from_secs(30));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(&pid_file).expect("pid");
        drop(run);
        assert!(
            !process_exists(pid.trim()),
            "the command must be gone when the drop returns"
        );
    }
}
