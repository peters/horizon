//! The first invitation of a server: the mode that a person runs on the server
//! computer itself, before any computer is logged in.
//!
//! The mode reaches the server through the Unix socket of its home, exactly as the
//! `ssh` link of a computer does, and it asks for one invitation. It starts no
//! server and reads no setting: a home without a server answers one typed line,
//! and a server that has no listener answers that it cannot hand out a link.
//!
//! The answer is one link, `horizon://<host>:<port>/join#<CODE>`, which is printed
//! with its code and as a QR code, so a person reads it on a screen and types it,
//! or shows it to the camera of another computer.

use std::{
    io,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use crate::frame::{read_frame, write_frame};
use crate::host::ServerHome;
use crate::protocol::{Frame, Hello, Invite, Invited, PROTOCOL_VERSION};

/// The device name that the server computer takes on its own server, when the mode
/// asks for the first invitation.
pub const SERVER_DEVICE: &str = "server";

/// Why the mode could not make an invitation.
#[derive(Debug, thiserror::Error)]
pub enum InviteError {
    /// No server answers the socket of this home.
    #[error("no Horizon server answers at {path}; start `horizon --server` on that computer")]
    NoServer {
        /// The socket path that the mode tried.
        path: PathBuf,
    },
    /// The socket failed.
    #[error("the server socket at {path}: {error}")]
    Io {
        /// The path of the socket.
        path: PathBuf,
        /// The reason that the system gave.
        error: io::Error,
    },
    /// The frames of the answer are not the ones that the mode asked for.
    #[error("the answer of the server is not one invitation: {0}")]
    Frame(String),
    /// The server refused the request with one typed code.
    #[error("{sentence}")]
    Refused {
        /// The stable code of the refusal.
        code: &'static str,
        /// One sentence for the person.
        sentence: String,
    },
}

impl InviteError {
    /// The one sentence of a refusal of the server, by its code.
    fn refused(code: &'static str, message: &str) -> Self {
        let sentence = match code {
            "no_listener" => "This server has no listener, so it cannot hand out a link. Start it with `horizon --server --listen <ip>:<port>`.".to_owned(),
            "protocol_version" => "This Horizon speaks another protocol than the running server. Update the binary of the server and start it again.".to_owned(),
            "not_hello" | "bad_frame" => format!("The server did not read the request: {message}"),
            _ => format!("The server refused the request: {message}"),
        };
        Self::Refused { code, sentence }
    }
}

/// Asks the server of `home` for one invitation, as the computer that runs it.
///
/// # Errors
/// Returns why the socket cannot be reached, why the answer is not one invitation,
/// and every typed refusal of the server.
pub fn first_invitation(home: &ServerHome, device: &str) -> Result<Invited, InviteError> {
    horizon_core::ui_thread::blocking("the first invitation of a server is asked");
    let path = home.socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|error| map_error(&path, error))?;
    greet(&mut stream, &path, device)?;
    ask(&mut stream, &path)
}

/// Sends the hello of the server computer and reads the greet of the server.
///
/// # Errors
/// Returns why the frame cannot be written, and every typed refusal.
pub fn greet(stream: &mut UnixStream, path: &Path, device: &str) -> Result<(), InviteError> {
    let hello = Frame::Hello(Hello {
        protocol: PROTOCOL_VERSION,
        device: device.to_owned(),
        store: None,
        token: None,
    });
    write_frame(stream, &hello).map_err(|error| json_error(path, &error))?;
    match read_frame(stream).map_err(|error| json_error(path, &error))? {
        Some(Frame::HelloOk(_)) => Ok(()),
        Some(Frame::Error(error)) => Err(InviteError::refused(error.code.as_str(), &error.message)),
        _ => Err(InviteError::Frame("the greet answered another frame".to_owned())),
    }
}

/// Asks one greeted link for one invitation.
///
/// # Errors
/// Returns why the request cannot be written, and every typed refusal.
pub fn ask(stream: &mut UnixStream, path: &Path) -> Result<Invited, InviteError> {
    write_frame(stream, &Frame::Invite(Invite {})).map_err(|error| json_error(path, &error))?;
    match read_frame(stream).map_err(|error| json_error(path, &error))? {
        Some(Frame::Invited(invited)) => Ok(invited),
        Some(Frame::Error(error)) => Err(InviteError::refused(error.code.as_str(), &error.message)),
        Some(_) => Err(InviteError::Frame("another frame".to_owned())),
        None => Err(InviteError::Frame("the link ended".to_owned())),
    }
}

/// The text that the mode of the first invitation prints: the link, its code, the
/// minutes that it lives and the QR code of the link.
///
/// # Errors
/// Returns why the link cannot be one QR code.
pub fn invitation_text(invited: &Invited) -> Result<String, horizon_core::QrError> {
    let qr = horizon_core::Qr::encode(&invited.link)?;
    let minutes = invited.expires_millis / 60_000;
    Ok(format!(
        "The first invitation of this Horizon server.\n\n\
         link: {}\n\
         code: {}\n\n\
         This invitation works for one computer, for {minutes} minutes.\n\n\
         {}",
        invited.link,
        invited.code,
        qr.text()
    ))
}

fn json_error(path: &Path, error: &crate::frame::FrameError) -> InviteError {
    match error {
        crate::frame::FrameError::Io(io_error) => {
            map_error(path, io::Error::new(io_error.kind(), io_error.to_string()))
        }
        other => InviteError::Frame(other.to_string()),
    }
}

fn map_error(path: &Path, error: io::Error) -> InviteError {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset => {
            InviteError::NoServer {
                path: path.to_path_buf(),
            }
        }
        _ => InviteError::Io {
            path: path.to_path_buf(),
            error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_of_a_server_without_a_listener_is_one_sentence() {
        let error = InviteError::refused("no_listener", "the server has no listener");
        assert_eq!(error.to_string(), "This server has no listener, so it cannot hand out a link. Start it with `horizon --server --listen <ip>:<port>`.".to_owned());
        match error {
            InviteError::Refused { code, .. } => assert_eq!(code, "no_listener"),
            other => panic!("{other}"),
        }
        match map_error(
            Path::new("/nowhere/server.sock"),
            io::Error::from(io::ErrorKind::ConnectionRefused),
        ) {
            InviteError::NoServer { path } => assert_eq!(path, Path::new("/nowhere/server.sock")),
            other => panic!("{other}"),
        }
        assert!(SERVER_DEVICE.len() <= 40);
    }

    #[test]
    fn the_mode_prints_a_link_that_the_parser_of_a_computer_accepts() {
        let invited = Invited {
            code: "ABCD-EFGH-JKMN".to_owned(),
            link: "horizon://server-computer:7000/join#ABCD-EFGH-JKMN".to_owned(),
            expires_millis: 600_000,
            now_millis: 1_760_000_000_000,
        };
        let text = invitation_text(&invited).expect("the text of one invitation");
        let link = text
            .lines()
            .find_map(|line| line.strip_prefix("link: "))
            .expect("the text holds the link");
        let parsed = horizon_core::JoinLink::parse(link).expect("the parser of a computer accepts the link");
        assert_eq!(parsed.code(), invited.code);
        assert_eq!(parsed.host(), "server-computer");
        assert!(text.contains("code: ABCD-EFGH-JKMN"), "{text}");
        assert!(text.contains("for 10 minutes"), "{text}");
        assert!(
            text.lines().any(|line| line.contains('\u{2588}')),
            "the text holds the dark modules of the QR code"
        );
    }
}
