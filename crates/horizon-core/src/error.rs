use std::path::PathBuf;

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

    /// The YAML settings file of an older Horizon cannot be read. The start stops
    /// and names the file, because the defaults must not replace the settings of
    /// a person.
    #[error("cannot read the settings of {}: {reason}", path.display())]
    OldSettingsFile { path: PathBuf, reason: String },

    #[error("Editor error: {0}")]
    Editor(String),

    #[error("Browser error: {0}")]
    Browser(String),

    #[error("Git error: {0}")]
    Git(String),

    /// A new panel of a workspace's place cannot be made: its machine is gone
    /// from the machine list, or its record no longer passes its rule.
    #[error("{0}")]
    Place(#[from] crate::place::PlaceError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
