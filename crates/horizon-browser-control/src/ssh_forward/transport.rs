//! One bounded `ssh` call and the rules that keep every value inside one word.
//!
//! The calls are one `ssh` process each, with nothing interactive: a target that
//! asks for a password would wait for input that never comes, so a person
//! connects once from a terminal, after which the key answers.

use std::io::{self, Read as _};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Longest SSH target that a call takes. It is one argument of `ssh`, so a short
/// alias is enough for an SSH config name, a `user@host` and an address.
const MAX_TARGET_LEN: usize = 255;

const SSH: &str = "ssh";
const READ_BUFFER: usize = 8 * 1024;
/// Bound of the TCP connect of one `ssh`, in seconds. A host that drops packets
/// fails here instead of holding the caller for the two minutes of a default
/// connect.
pub(super) const CONNECT_TIMEOUT_SECONDS: u32 = 5;
/// Keepalive interval of one `ssh`, in seconds, so a host that stops answering
/// after the connect fails at the same order of time.
pub(super) const SERVER_ALIVE_INTERVAL_SECONDS: u32 = 5;
/// Number of keepalives without an answer that end one `ssh`.
pub(super) const SERVER_ALIVE_COUNT_MAX: u32 = 2;

/// Whether `target` is one name that `ssh` takes as one argument and not as an
/// option of its own.
#[must_use]
pub fn valid_target(target: &str) -> bool {
    let target = target.trim();
    !target.is_empty()
        && target.len() <= MAX_TARGET_LEN
        && !target.starts_with('-')
        && !target.chars().any(char::is_whitespace)
        && !target.chars().any(char::is_control)
}

/// The program of one `ssh` call.
#[must_use]
pub fn ssh_program() -> String {
    SSH.to_string()
}

/// One `ssh` call with every option that keeps it from waiting: no prompt, a
/// bounded connect and a bounded keepalive, so a host that drops packets or asks
/// a question fails in seconds instead of holding the caller.
///
/// The log level is `ERROR` and not the quieter `-q`, which is `QUIET` and drops
/// the fatal line as well: a target that `ssh` could not resolve then reported
/// an empty reason, so a refusal said nothing that a person could act on.
///
/// The command is one argument that the remote shell reads, so a caller keeps
/// every value of it inside the allowed words of that shell.
#[must_use]
pub fn ssh_command(program: &str, target: &str, remote_command: &str) -> Command {
    let mut command = Command::new(program);
    command.args([
        "-T".to_string(),
        "-o".to_string(),
        "LogLevel=ERROR".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
        "-o".to_string(),
        format!("ServerAliveInterval={SERVER_ALIVE_INTERVAL_SECONDS}"),
        "-o".to_string(),
        format!("ServerAliveCountMax={SERVER_ALIVE_COUNT_MAX}"),
        target.to_string(),
        remote_command.to_string(),
    ]);
    command
}

/// Why one `ssh` call failed: its own words, or its exit status when it wrote
/// none. A refusal never ends in an empty reason, because an operator cannot act
/// on one.
pub(super) fn failure_reason(code: Option<i32>, stderr: &str) -> String {
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    match code {
        Some(code) => format!("{SSH} exited with status {code} and wrote no reason"),
        None => format!("{SSH} ended on a signal and wrote no reason"),
    }
}

/// What one finished `ssh` call gives back. `too_large` says that the call wrote
/// more than the caller asked to read, so its output was stopped early.
pub(super) struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub too_large: bool,
}

impl Output {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0) && !self.too_large
    }
}

/// Run one command and read at most `limit` bytes of its standard output, with a
/// deadline. The read runs on its own thread, so a call that never answers is
/// killed when the deadline passes and the caller gets
/// [`io::ErrorKind::TimedOut`] instead of a wait.
pub(super) fn run_bounded(mut command: Command, limit: u64, deadline: Duration) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err(io::Error::other("ssh has no standard output"));
    };
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; READ_BUFFER];
        let mut too_large = false;
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if u64::try_from(bytes.len() + read).unwrap_or(u64::MAX) > limit {
                        too_large = true;
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
            }
        }
        // The receiver may be gone after a deadline, which is not an error.
        let _ = sender.send((bytes, too_large));
    });
    let Ok((bytes, too_large)) = received.recv_timeout(deadline) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("ssh gave no answer within {} seconds", deadline.as_secs()),
        ));
    };
    if too_large {
        let _ = child.kill();
    }
    let finished = child.wait_with_output()?;
    Ok(Output {
        code: finished.status.code(),
        stdout: bytes,
        stderr: String::from_utf8_lossy(&finished.stderr).trim().to_string(),
        too_large,
    })
}

/// A value as the remote shell must read it: one word, with no expansion.
#[must_use]
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
