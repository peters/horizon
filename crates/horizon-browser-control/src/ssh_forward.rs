//! One loopback forward to the `DevTools` port of a browser on another machine.
//!
//! The browser of that machine listens on its own loopback, so this module
//! carries that port to this computer through one `ssh -N -L` child process. It
//! is the whole transport: the far side needs `sh`, `cat`, `mkdir` and `nohup`,
//! and no Horizon program, record file or interactive prompt takes part.
//!
//! The two snippets at the end start a Chromium of that machine in the profile
//! folder of the named profile, detached so that a signed-in page survives this
//! computer, and read the endpoint that Chromium writes there. The local side
//! decides whether to reuse a browser: it reads the endpoint first
//! ([`remote_port_command`]) and starts one only when no endpoint is there.

use std::io;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod remote_browser;
mod ssh;
mod transport;

pub use horizon_browser::valid_profile_name as valid_profile;
pub use remote_browser::{remote_browser_command, remote_port_command};
pub use ssh::Ssh;
pub use transport::{ssh_command, ssh_program, valid_target};

use self::transport::{
    CONNECT_TIMEOUT_SECONDS, SERVER_ALIVE_COUNT_MAX, SERVER_ALIVE_INTERVAL_SECONDS, failure_reason, run_bounded,
};

/// Bound of the readiness of one forward. It holds the SSH connect, the
/// authentication and the bind on the far side.
pub const READY_DEADLINE: Duration = Duration::from_secs(20);
/// Bound of the TCP connect that asks whether the forward takes a connection.
const READY_PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const READY_POLL: Duration = Duration::from_millis(50);
/// Number of forwards that one start makes when `ssh` reports the local port in
/// use: the port is released before the child binds it, so another process can
/// take it in that window.
const FORWARD_ATTEMPTS: usize = 2;
/// Lines of `ssh` that a failure keeps, newest last, and the bound of one line.
const STDERR_TAIL_LINES: usize = 8;
const STDERR_LINE_CHARS: usize = 400;
/// Bound of the wait for the last words of `ssh` after it exited.
const STDERR_GRACE: Duration = Duration::from_secs(1);
const REAP_TIMEOUT: Duration = Duration::from_secs(3);
const REAP_POLL: Duration = Duration::from_millis(20);
/// Bound of the answer of one one-shot command on the remote machine.
const REMOTE_OUTPUT_LIMIT: u64 = 64 * 1024;

/// Why this computer cannot reach the browser of another machine.
#[derive(Debug, thiserror::Error)]
pub enum SshForwardError {
    #[error("`{target}` is not one SSH name that `ssh` can take as one argument")]
    InvalidTarget { target: String },
    #[error("TCP port {port} is not one port that a forward can take")]
    InvalidPort { port: u16 },
    #[error("could not start the SSH forward to {target}: {reason}")]
    Spawn { target: String, reason: String },
    #[error("the SSH forward to {target} ended before 127.0.0.1:{local_port} took a connection: {reason}")]
    ForwardEnded {
        target: String,
        local_port: u16,
        reason: String,
    },
    #[error(
        "the SSH forward to {target} did not take a connection on 127.0.0.1:{local_port} within {seconds} seconds: {reason}"
    )]
    NotListening {
        target: String,
        local_port: u16,
        seconds: u64,
        reason: String,
    },
    #[error("the command on {target} failed: {reason}")]
    RemoteCommandFailed {
        target: String,
        /// Exit code of the command on that machine. A snippet names its own
        /// refusal with one, and `ssh` itself refuses with 255.
        code: Option<i32>,
        reason: String,
    },
    #[error("the command on {target} gave no answer within {seconds} seconds")]
    RemoteNoAnswer { target: String, seconds: u64 },
}

impl SshForwardError {
    /// Whether `ssh` refused the local port because another process took it
    /// between its release and the bind, which one fresh port answers.
    fn is_local_port_taken(&self) -> bool {
        let reason = match self {
            Self::ForwardEnded { reason, .. } | Self::NotListening { reason, .. } => reason,
            Self::InvalidTarget { .. }
            | Self::InvalidPort { .. }
            | Self::Spawn { .. }
            | Self::RemoteCommandFailed { .. }
            | Self::RemoteNoAnswer { .. } => return false,
        };
        reason.to_ascii_lowercase().contains("address already in use")
    }
}

/// One `ssh -N -L` child process that carries 127.0.0.1:{local} of this computer
/// to 127.0.0.1:{remote} of another machine. It ends with this value.
#[derive(Debug)]
pub struct SshForward {
    child: Child,
    local_port: u16,
}

impl SshForward {
    /// Forward one free loopback port of this computer to the `DevTools` port
    /// of `target`, and wait until the forward takes a connection.
    ///
    /// # Errors
    /// `target` is not one SSH name, no loopback port is free, `ssh` cannot
    /// start, it ends before the forward listens (a refused host key names
    /// itself in the reason), or no connection arrives within
    /// [`READY_DEADLINE`].
    pub fn start(target: &str, remote_port: u16) -> Result<Self, SshForwardError> {
        Self::start_with(target, remote_port, &ssh_program(), READY_DEADLINE, accepts_connection)
    }

    /// The loopback port of this computer that the forward listens on.
    #[must_use]
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// [`Self::start`] with the program and the deadline of a test.
    fn start_with(
        target: &str,
        remote_port: u16,
        program: &str,
        ready_deadline: Duration,
        mut is_ready: impl FnMut(u16) -> bool,
    ) -> Result<Self, SshForwardError> {
        let target = target.trim();
        if !valid_target(target) {
            return Err(SshForwardError::InvalidTarget {
                target: target.to_string(),
            });
        }
        if remote_port == 0 {
            return Err(SshForwardError::InvalidPort { port: remote_port });
        }
        let mut attempt = 1;
        loop {
            let error = match Self::start_once(target, remote_port, program, ready_deadline, &mut is_ready) {
                Ok(forward) => return Ok(forward),
                Err(error) => error,
            };
            if attempt >= FORWARD_ATTEMPTS || !error.is_local_port_taken() {
                return Err(error);
            }
            attempt += 1;
        }
    }

    fn start_once(
        target: &str,
        remote_port: u16,
        program: &str,
        ready_deadline: Duration,
        is_ready: &mut impl FnMut(u16) -> bool,
    ) -> Result<Self, SshForwardError> {
        let local_port = free_loopback_port(target)?;
        let mut child = forward_command(program, target, local_port, remote_port)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| SshForwardError::Spawn {
                target: target.to_string(),
                reason: error.to_string(),
            })?;
        let Some(stderr) = child.stderr.take() else {
            kill_and_reap(&mut child);
            return Err(SshForwardError::Spawn {
                target: target.to_string(),
                reason: "ssh has no standard error".to_string(),
            });
        };
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&stderr_tail);
        let reader = std::thread::Builder::new()
            .name("ssh-forward-stderr".to_string())
            .spawn(move || collect_stderr(stderr, &collected));
        if let Err(error) = reader {
            kill_and_reap(&mut child);
            return Err(SshForwardError::Spawn {
                target: target.to_string(),
                reason: error.to_string(),
            });
        }
        let deadline = Instant::now() + ready_deadline;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(SshForwardError::ForwardEnded {
                        target: target.to_string(),
                        local_port,
                        reason: failure_reason(status.code(), &ssh_words(&stderr_tail)),
                    });
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not read the state of the ssh forward"),
            }
            if is_ready(local_port) {
                return Ok(Self { child, local_port });
            }
            if Instant::now() >= deadline {
                let reason = silent_reason(&stderr_tail);
                kill_and_reap(&mut child);
                return Err(SshForwardError::NotListening {
                    target: target.to_string(),
                    local_port,
                    seconds: ready_deadline.as_secs(),
                    reason,
                });
            }
            std::thread::sleep(READY_POLL);
        }
    }
}

impl Drop for SshForward {
    fn drop(&mut self) {
        kill_and_reap(&mut self.child);
    }
}

/// Whether the forward of `port` takes a connection, which is what makes it
/// ready: `ssh` reports a successful forward before it can serve it.
fn accepts_connection(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), READY_PROBE_TIMEOUT).is_ok()
}

/// One `ssh` that forwards one loopback port and holds the session open, with
/// nothing interactive: no terminal, a bounded connect and keepalive, and an
/// exit when the far side refuses the forward. A forward that `ssh` could not
/// establish would otherwise look like a slow host.
fn forward_command(program: &str, target: &str, local_port: u16, remote_port: u16) -> Command {
    let mut command = Command::new(program);
    command.args([
        "-N".to_string(),
        "-L".to_string(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
        "-T".to_string(),
        "-o".to_string(),
        "LogLevel=ERROR".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECONDS}"),
        "-o".to_string(),
        format!("ServerAliveInterval={SERVER_ALIVE_INTERVAL_SECONDS}"),
        "-o".to_string(),
        format!("ServerAliveCountMax={SERVER_ALIVE_COUNT_MAX}"),
        target.to_string(),
    ]);
    command
}

/// A free loopback port of this computer, taken from the kernel and released at
/// once for `ssh` to bind.
fn free_loopback_port(target: &str) -> Result<u16, SshForwardError> {
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| SshForwardError::Spawn {
            target: target.to_string(),
            reason: format!("no free loopback port: {error}"),
        })?;
    Ok(port)
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(REAP_POLL),
            Ok(None) => {
                tracing::warn!(pid = ?child.id(), "the ssh forward did not exit after kill");
                return;
            }
        }
    }
}

fn collect_stderr(stderr: ChildStderr, tail: &Arc<Mutex<Vec<String>>>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut tail = tail.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tail.len() == STDERR_TAIL_LINES {
            tail.remove(0);
        }
        tail.push(line.chars().take(STDERR_LINE_CHARS).collect());
    }
}

/// The last words of `ssh`, once it wrote them. The reader thread of the child
/// may still be behind a just-ended `ssh`, so the wait is bounded.
fn ssh_words(tail: &Arc<Mutex<Vec<String>>>) -> String {
    let deadline = Instant::now() + STDERR_GRACE;
    loop {
        let words = tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("; ");
        if !words.is_empty() || Instant::now() >= deadline {
            return words;
        }
        std::thread::sleep(READY_POLL);
    }
}

/// Why a forward that `ssh` still holds took no connection.
fn silent_reason(tail: &Arc<Mutex<Vec<String>>>) -> String {
    let words = ssh_words(tail);
    if words.is_empty() {
        "ssh wrote no reason".to_string()
    } else {
        words
    }
}

/// Run one command on `target` and give its standard output. This is how this
/// computer reads the `DevTools` endpoint of that machine and starts its browser.
///
/// # Errors
/// `target` is not one SSH name, `ssh` cannot start or fails (its words are the
/// reason), or the command writes nothing before `deadline`.
pub fn run_remote(target: &str, command: &str, deadline: Duration) -> Result<String, SshForwardError> {
    run_remote_with(target, command, &ssh_program(), REMOTE_OUTPUT_LIMIT, deadline)
}

fn run_remote_with(
    target: &str,
    command: &str,
    program: &str,
    limit: u64,
    deadline: Duration,
) -> Result<String, SshForwardError> {
    let target = target.trim();
    if !valid_target(target) {
        return Err(SshForwardError::InvalidTarget {
            target: target.to_string(),
        });
    }
    let output = run_bounded(ssh_command(program, target, command), limit, deadline).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            return SshForwardError::RemoteNoAnswer {
                target: target.to_string(),
                seconds: deadline.as_secs(),
            };
        }
        SshForwardError::RemoteCommandFailed {
            target: target.to_string(),
            code: None,
            reason: error.to_string(),
        }
    })?;
    if !output.succeeded() {
        return Err(SshForwardError::RemoteCommandFailed {
            target: target.to_string(),
            code: output.code,
            reason: failure_reason(output.code, &output.stderr),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests;
