use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("PTY error: {0}")]
    Pty(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("State error: {0}")]
    State(String),

    /// The board of a saved session is not one this Horizon can read. A later Horizon
    /// wrote it, or it names something this Horizon does not have. The session stays
    /// whole until a person asks for it to go with its board.
    #[error("the board of session {session_id} is not one this Horizon can read: {reason}")]
    UnreadableBoard { session_id: String, reason: String },

    #[error("Editor error: {0}")]
    Editor(String),

    #[error("Browser error: {0}")]
    Browser(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
