//! The two transports of one link of a computer: one `ssh` child, or one socket to
//! the TCP listener of a server.
//!
//! Both answer two halves that carry the same frames, so the supervisor above them
//! has one code path. The differences are the ones that a person sees: `ssh`
//! starts a program of this computer, and the socket resolves a name and checks the
//! address rule of the link before it sends one byte.
//!
//! The socket is made blocking, because the reader thread of a link waits in a
//! read that nothing of this process ends. A link that reaches its bound is ended
//! by a shutdown of that socket, which makes the read return at once.

use std::{
    io::{BufRead, BufReader, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use horizon_core::server_link::ServerAddress;

use crate::refusal::{LinkRefusal, RefusalCell, drain_within, of_ssh_line};
use crate::supervisor::MAX_STDERR_LINE;

/// How long one address of the server may take to accept a link.
const CONNECT_BOUND: Duration = Duration::from_secs(5);

/// What a supervisor must end when a link is over or reaches its bound.
#[derive(Debug)]
pub(super) enum Halt {
    /// The `ssh` child of the link, with the thread that reads its words.
    Child {
        child: Child,
        /// The reader of the standard error of the child, joined before the reason
        /// of the attempt is read.
        words: Option<thread::JoinHandle<()>>,
    },
    /// The socket of the listener, which a shutdown makes readable at once.
    Socket(TcpStream),
}

impl Halt {
    /// Ends the process or the socket, so a read that waits returns.
    pub(super) fn end(&mut self) {
        match self {
            Self::Child { child, .. } => {
                let _ = child.kill();
            }
            Self::Socket(socket) => {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }

    /// Waits for the process that ended, so no zombie stays behind, and answers
    /// the exit status that names a reason of its own.
    pub(super) fn reap(self) -> Option<i32> {
        let Self::Child { mut child, words } = self else {
            return None;
        };
        let status = child.wait().ok().and_then(|status| status.code());
        // The words of the child decide the reason of this attempt, so the thread
        // that reads them ends first. A pipe that a process of the far side still
        // holds never keeps the link: the wait is bounded.
        if let Some(words) = words.as_ref() {
            drain_within(|| words.is_finished());
        }
        status
    }
}

/// The two halves of one open link.
pub(super) struct Opened {
    /// The frames of this computer leave here.
    pub(super) writer: Box<dyn Write + Send>,
    /// The frames of the server arrive at this stream, which the reader thread of
    /// the link reads.
    pub(super) answers: Box<dyn std::io::Read + Send>,
    /// What the supervisor ends to stop the link.
    pub(super) halt: Halt,
}

/// Starts one `ssh` child and its two pipes.
pub(super) fn ssh(command: &mut Command, program: &str, refusal: &RefusalCell) -> Result<Opened, String> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start `{program}`: {error}"))?;
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("the child of the link has no standard input or output".to_owned());
    };
    let words = child.stderr.take().and_then(|stderr| {
        let refusal = refusal.clone();
        thread::Builder::new()
            .name("horizon-sync-stderr".to_owned())
            .spawn(move || log_stderr(stderr, &refusal))
            .ok()
    });
    Ok(Opened {
        writer: Box::new(input),
        answers: Box::new(output),
        halt: Halt::Child { child, words },
    })
}

/// Opens one socket to the TCP listener of `address`.
///
/// Every address that the name of the server resolves to passes the rule of the
/// link before this call sends one byte, and the fields of the address decide what
/// the rule is. A name that resolves outside the three ranges of a link is one
/// plain sentence, and no connection is made.
pub(super) fn tcp(address: &str, refusal: &RefusalCell) -> Result<Opened, String> {
    let address = ServerAddress::parse(address).map_err(|error| {
        refusal.set(LinkRefusal::Other);
        error.to_string()
    })?;
    let resolved = resolve(&address, refusal)?;
    let mut last = None;
    for socket in resolved {
        horizon_core::check_address(socket.ip()).map_err(|error| error.to_string())?;
        match TcpStream::connect_timeout(&socket, CONNECT_BOUND) {
            Ok(stream) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|error| format!("the link to {socket}: {error}"))?;
                let _ = stream.set_nodelay(true);
                let answers = stream
                    .try_clone()
                    .map_err(|error| format!("the link to {socket}: {error}"))?;
                let halt = stream
                    .try_clone()
                    .map_err(|error| format!("the link to {socket}: {error}"))?;
                return Ok(Opened {
                    writer: Box::new(stream),
                    answers: Box::new(answers),
                    halt: Halt::Socket(halt),
                });
            }
            Err(error) => {
                refusal.set(LinkRefusal::Address);
                last = Some(format!("{socket}: {error}"));
            }
        }
    }
    Err(match last {
        Some(reason) => format!("the server at {}: {reason}", address.authority()),
        None => format!("the server at {} answers no address", address.authority()),
    })
}

/// Every address of `address`, which all pass the rule of the link.
fn resolve(address: &ServerAddress, refusal: &RefusalCell) -> Result<Vec<SocketAddr>, String> {
    let name = address.authority();
    let resolved: Vec<SocketAddr> = name
        .to_socket_addrs()
        .map_err(|error| {
            refusal.set(LinkRefusal::Name);
            format!("the name {name}: {error}")
        })?
        .collect();
    if resolved.is_empty() {
        refusal.set(LinkRefusal::Name);
        return Err(format!("the name {name} answers no address"));
    }
    for socket in &resolved {
        horizon_core::check_address(socket.ip()).map_err(|error| error.to_string())?;
    }
    Ok(resolved)
}

/// Logs the standard error of the child, a line at a time, and keeps the reason
/// that a line names. A pipe that nobody reads stops the child, so this thread
/// runs for every link.
fn log_stderr(stderr: impl std::io::Read, refusal: &RefusalCell) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { return };
        let line = line.chars().take(MAX_STDERR_LINE).collect::<String>();
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reason) = of_ssh_line(&line) {
            refusal.set(reason);
        }
        tracing::warn!(target: "horizon_sync::ssh", "{line}");
    }
}
