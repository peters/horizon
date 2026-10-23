use crate::LayoutError;

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
    /// A tunnel failed; the text includes its last diagnostic lines.
    #[error("{0}")]
    Tunnel(String),
    #[error("connection timed out")]
    Timeout,
}

impl From<vnc::VncError> for Error {
    fn from(error: vnc::VncError) -> Self {
        Self::Protocol(error.to_string())
    }
}
