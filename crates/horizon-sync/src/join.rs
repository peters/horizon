//! The computer side of a join: one connection to the TCP listener of a server,
//! one code in, one token back.
//!
//! A person pastes the link that a logged-in computer showed, and this module is
//! everything that happens next. The text is untrusted, so its parser is
//! [`horizon_core::server_link::JoinLink`], and every address that the name of the
//! server resolves to passes the address rule of the link before this module sends
//! one byte.
//!
//! The answer is one token, which is written to the secrets file of this computer
//! under the id `server/token`. From then on the link of this computer presents
//! that token in every hello.

use std::{
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

use horizon_core::secret_store::{self, SecretStore};
use horizon_core::server_link::{JoinLink, LinkError};

use crate::frame::{read_frame, write_frame};
use crate::protocol::{Frame, Join, Joined};

/// How long one address of the server may take to accept a join.
const CONNECT_BOUND: Duration = Duration::from_secs(10);

/// How long the server may take to answer a join once the link is open. A server
/// that accepts the link and answers nothing is a server that no join waits for.
const ANSWER_BOUND: Duration = Duration::from_secs(20);

/// The bounds of one join, so a test sees a join that the server kept silent
/// within a second instead of a minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinBounds {
    /// How long one address of the server may take to accept the link.
    pub connect: Duration,
    /// How long the server may take to answer the join once the link is open.
    pub answer: Duration,
}

impl JoinBounds {
    /// The bounds of a real join.
    pub const STANDARD: Self = Self {
        connect: CONNECT_BOUND,
        answer: ANSWER_BOUND,
    };
}

impl Default for JoinBounds {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// Why this computer did not join a server.
#[derive(Debug, thiserror::Error)]
pub enum JoinError {
    /// The text that a person pasted is not one join link.
    #[error(transparent)]
    Link(#[from] LinkError),
    /// The name of this computer is not one device name.
    #[error("The name of this computer is 1 to 40 characters of a-z, 0-9 and -, with no - at either end.")]
    Device,
    /// The name of the server does not resolve, or no address of it answers.
    #[error("{0}")]
    Address(String),
    /// The link to the server failed.
    #[error("{0}")]
    Io(String),
    /// The server refused the join with one typed code.
    #[error("{sentence}")]
    Refused {
        /// The stable code of the refusal, for a log line.
        code: &'static str,
        /// One sentence for the person.
        sentence: String,
    },
    /// The token could not be written to the secrets file of this computer.
    #[error("The token of this computer could not be written: {0}")]
    Token(String),
    /// The server accepted the link and answered nothing inside the bound.
    #[error(
        "The server did not answer the join in time. Try again, or make a new invitation on a computer that is logged in."
    )]
    Timeout,
}

impl JoinError {
    /// The stable code of this error, for a log line and a test.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Link(error) => error.code(),
            Self::Device => "bad_device",
            Self::Address(_) => "bad_address",
            Self::Io(_) => "link_failed",
            Self::Refused { code, .. } => code,
            Self::Token(_) => "token_not_written",
            Self::Timeout => "join_timeout",
        }
    }

    /// One sentence for the person, which is the text of this error.
    #[must_use]
    pub fn sentence(&self) -> String {
        self.to_string()
    }
}

/// The one sentence of a refusal of the server, by its code.
fn sentence_of(code: &str, message: &str) -> String {
    match code {
        "wrong_code" => "The code is not one open invitation of this server. Check the code, or make a new invitation on a computer that is logged in.".to_owned(),
        "join_locked" => "Too many wrong codes. Every invitation is gone, so make a new one on a computer that is logged in.".to_owned(),
        "name_taken" => "This server already has a computer with that name. Remove the old one first, and then try again.".to_owned(),
        "bad_device" => "The name of this computer is 1 to 40 characters of a-z, 0-9 and -, with no - at either end.".to_owned(),
        "protocol_version" => "This Horizon speaks another protocol than the server. Update this computer and try again.".to_owned(),
        "not_logged_in" => "This computer is not logged in to that server. Join it with a link from a computer that is logged in.".to_owned(),
        "frame_too_large" | "bad_frame" | "not_hello" => format!("That server did not read the join: {message}"),
        _ => format!("That server refused the join: {message}"),
    }
}

/// Joins the server of `text` as `device`, and writes the token to `store`.
///
/// This connects and reads a file, so it runs on a worker thread and never on the
/// UI thread.
///
/// # Errors
/// Refuses text that is not one join link, an address outside the ranges of a link,
/// and every typed refusal of the server.
pub fn join(text: &str, device: &str, store: &SecretStore) -> Result<Joined, JoinError> {
    join_with(text, device, store, JoinBounds::STANDARD)
}

/// [`join`] with the bounds of one test.
///
/// # Errors
/// Refuses text that is not one join link, an address outside the ranges of a link,
/// and every typed refusal of the server.
pub fn join_with(text: &str, device: &str, store: &SecretStore, bounds: JoinBounds) -> Result<Joined, JoinError> {
    horizon_core::ui_thread::blocking("the join of a server is made");
    let link = JoinLink::parse(text)?;
    if !crate::protocol::check_device_name(device) {
        return Err(JoinError::Device);
    }
    let resolved = resolve(&link)?;
    let mut last = None;
    for socket in resolved {
        horizon_core::check_address(socket.ip())?;
        match TcpStream::connect_timeout(&socket, bounds.connect) {
            Ok(mut stream) => {
                let _ = stream.set_nonblocking(false);
                let bound = Some(bounds.answer);
                stream
                    .set_read_timeout(bound)
                    .and_then(|()| stream.set_write_timeout(bound))
                    .map_err(|error| JoinError::Io(error.to_string()))?;
                return exchange(&mut stream, &link, device, store);
            }
            Err(error) => last = Some(format!("{socket}: {error}")),
        }
    }
    Err(JoinError::Address(match last {
        Some(reason) => format!("The server at {} did not answer: {reason}", link.authority()),
        None => format!("The server at {} answers no address.", link.authority()),
    }))
}

/// Every address of the name of `link`, which all pass the rule of the link.
fn resolve(link: &JoinLink) -> Result<Vec<SocketAddr>, JoinError> {
    if let Some(socket) = link.socket_addr() {
        horizon_core::check_address(socket.ip())?;
        return Ok(vec![socket]);
    }
    let name = link.authority();
    let resolved: Vec<SocketAddr> = name
        .to_socket_addrs()
        .map_err(|error| JoinError::Address(format!("The name {name} did not resolve: {error}")))?
        .collect();
    if resolved.is_empty() {
        return Err(JoinError::Address(format!("The name {name} answers no address.")));
    }
    for socket in &resolved {
        horizon_core::check_address(socket.ip())?;
    }
    Ok(resolved)
}

/// Sends one join over an open socket and reads its answer.
fn exchange(stream: &mut TcpStream, link: &JoinLink, device: &str, store: &SecretStore) -> Result<Joined, JoinError> {
    let join = Frame::Join(Join {
        code: link.code().to_owned(),
        device: device.to_owned(),
    });
    write_frame(stream, &join).map_err(|error| frame_error(&error))?;
    let answer = read_frame(stream).map_err(|error| frame_error(&error))?;
    match answer {
        Some(Frame::Joined(joined)) => {
            if joined.device != device {
                return Err(JoinError::Refused {
                    code: "bad_device",
                    sentence: "That server answered another name for this computer. Try again.".to_owned(),
                });
            }
            if !horizon_core::secret_store::check_token(&joined.token) {
                return Err(JoinError::Refused {
                    code: "bad_token",
                    sentence: "That server answered a token that this Horizon does not read.".to_owned(),
                });
            }
            secret_store::write_token(store, &joined.token).map_err(|error| JoinError::Token(error.to_string()))?;
            Ok(joined)
        }
        Some(Frame::Error(error)) => Err(JoinError::Refused {
            code: error.code.as_str(),
            sentence: sentence_of(error.code.as_str(), &error.message),
        }),
        Some(frame) => Err(JoinError::Refused {
            code: "unexpected",
            sentence: format!("That server answered a {} frame to a join.", name_of(&frame)),
        }),
        None => Err(JoinError::Refused {
            code: "ended",
            sentence: "That server ended the link before it answered the join.".to_owned(),
        }),
    }
}

/// Why one frame of a join did not travel. A socket that reaches its bound reads
/// as the typed timeout, so a server that accepts the link and answers nothing
/// ends the join with a sentence of its own.
fn frame_error(error: &crate::frame::FrameError) -> JoinError {
    match error {
        crate::frame::FrameError::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            JoinError::Timeout
        }
        _ => JoinError::Io(error.to_string()),
    }
}

fn name_of(frame: &Frame) -> &'static str {
    match frame {
        Frame::Hello(_) => "hello",
        Frame::HelloOk(_) => "hello_ok",
        Frame::Join(_) => "join",
        Frame::Joined(_) => "joined",
        Frame::Invite(_) => "invite",
        Frame::Invited(_) => "invited",
        Frame::Devices(_) => "devices",
        Frame::DeviceList(_) => "device_list",
        Frame::DeviceRemove(_) => "device_remove",
        Frame::DeviceRemoved(_) => "device_removed",
        Frame::Push(_) => "push",
        Frame::Pushed(_) => "pushed",
        Frame::Pull(_) => "pull",
        Frame::Changes(_) => "changes",
        Frame::Wait(_) => "wait",
        Frame::Error(_) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_accepts_a_join_and_answers_nothing_ends_it_at_its_bound() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a listener");
        let address = listener.local_addr().expect("the address of the listener");
        let holding = std::thread::spawn(move || {
            // The link is accepted and held open without one frame of an answer.
            if let Ok((stream, _peer)) = listener.accept() {
                std::thread::sleep(Duration::from_secs(3));
                drop(stream);
            }
        });
        let directory = tempfile::tempdir().expect("a directory");
        let store = SecretStore::open(directory.path().join("secrets.sqlite")).expect("the secrets file");
        let link = format!("horizon://127.0.0.1:{}/join#ABCD-EFGH-JKMN", address.port());
        let started = std::time::Instant::now();
        let error = join_with(
            &link,
            "laptop-b",
            &store,
            JoinBounds {
                connect: Duration::from_secs(2),
                answer: Duration::from_millis(200),
            },
        )
        .expect_err("the server answered nothing");
        assert_eq!(error.code(), "join_timeout");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the join ends at its own bound"
        );
        assert!(
            secret_store::read_token(&store).expect("no token").is_none(),
            "a join that no answer reached writes no token"
        );
        holding.join().expect("the link of the test");
    }

    #[test]
    fn a_refusal_is_one_sentence_without_a_code() {
        for code in [
            "wrong_code",
            "join_locked",
            "name_taken",
            "bad_device",
            "protocol_version",
        ] {
            let sentence = sentence_of(code, "a message of the server");
            assert!(sentence.ends_with('.'), "{sentence}");
            assert!(!sentence.contains(code), "the person reads no code: {sentence}");
            assert!(sentence.len() < 190, "{sentence}");
        }
    }
}
