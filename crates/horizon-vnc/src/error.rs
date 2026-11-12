use crate::{LayoutError, session::HostKeyFailure};

/// Why a session stopped or a frame could not be made. The text is what a
/// viewer shows to a person.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The VNC protocol or decoder failed.
    #[error("{0}")]
    Protocol(String),
    /// The server sent a rectangle or a desktop size the framebuffer refuses.
    #[error("{0}")]
    Frame(&'static str),
    /// The server reported an error.
    #[error("{0}")]
    Server(String),
    /// The view options cannot make an image of this desktop.
    #[error("{0}")]
    Layout(#[from] LayoutError),
    /// A tunnel failed. `message` holds its last diagnostic lines and what a
    /// person can do next; `host_key` types a refusal of the host key. The
    /// refusal is boxed, so a failed result stays small.
    #[error("{message}")]
    Tunnel {
        message: String,
        host_key: Option<Box<HostKeyFailure>>,
    },
    #[error("connection timed out")]
    Timeout,
    /// The server, or the stream that carried it, ended the connection.
    #[error("The desktop closed the connection")]
    Closed,
}

impl Error {
    /// The host-key refusal that ended a tunnel, when ssh named one.
    #[must_use]
    pub fn host_key_failure(&self) -> Option<&HostKeyFailure> {
        match self {
            Self::Tunnel { host_key, .. } => host_key.as_deref(),
            _ => None,
        }
    }
}

impl From<vnc::VncError> for Error {
    fn from(error: vnc::VncError) -> Self {
        // The library text of a stopped client says nothing to a person, so it
        // stays in the log; a viewer reads one plain sentence for the cause.
        match error {
            vnc::VncError::ClientNotRunning => Self::Closed,
            vnc::VncError::IoError(error) => match error.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Self::Timeout,
                _ => Self::Io(error),
            },
            other => Self::Protocol(other.to_string()),
        }
    }
}
