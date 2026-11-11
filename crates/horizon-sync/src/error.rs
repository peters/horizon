//! Why the server refused one intent, and why the store could not answer.

use serde::{Deserialize, Serialize};

use crate::frame::FrameError;

/// Why the server refused one intent.
///
/// A refusal changes nothing. It is deterministic for the state that the server
/// has, so a computer drops the intent from its queue and reports the reason to
/// the person instead of sending it again.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, thiserror::Error)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    /// The object changed since the version that the computer saw.
    #[error("It changed since you saw it (version {seen}, now {version}). Look again and answer")]
    ChangedSinceSeen { seen: u64, version: u64 },
    /// Another intent already made this object, and no version was seen.
    #[error("That was already answered (version {version}). Pull the change and look again")]
    AlreadyAnswered { version: u64 },
    /// The kind holds the most objects that the server may keep.
    #[error("The server already keeps the maximum of {limit} {kind} objects")]
    TooMany { kind: String, limit: u64 },
    /// The record breaks a rule that Horizon applies to it.
    #[error("{message}")]
    InvalidRecord { code: String, message: String },
    /// The kind of object is not one that this server holds.
    #[error("The server holds no object of the kind {kind}")]
    UnknownKind { kind: String },
    /// The intent itself breaks a rule of the protocol.
    #[error("{message}")]
    InvalidIntent { message: String },
}

impl Refusal {
    /// The stable code of this refusal, for a tool answer or a log line.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ChangedSinceSeen { .. } => "changed_since_seen",
            Self::AlreadyAnswered { .. } => "already_answered",
            Self::TooMany { .. } => "too_many",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::UnknownKind { .. } => "unknown_kind",
            Self::InvalidIntent { .. } => "invalid_intent",
        }
    }
}

/// Why the store could not answer a caller.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The server file could not be opened, read or written.
    #[error("the server store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The server file is from a newer Horizon.
    #[error(
        "the server store has schema version {found}, newer than version {expected} of this Horizon; start the Horizon that made it"
    )]
    Newer { found: i64, expected: i64 },
    /// The server file is damaged, or it is not a Horizon server store.
    #[error("the server store is damaged: {0}")]
    Damaged(String),
    /// A file of the server home could not be read or written.
    #[error("{0}")]
    File(String),
    /// The writer task ended before it answered.
    #[error("the server store stopped before it answered; start the server again")]
    Stopped,
    /// The caller asked for more intents in one push than the protocol allows.
    #[error("one push holds at most {limit} intents")]
    TooManyIntents { limit: usize },
}

/// The errors of the computer side of a link.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The link failed.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The server refused the frame with a typed error.
    #[error("the server refused: {code}: {message}")]
    Refused { code: &'static str, message: String },
    /// The server answered a frame that this computer did not ask for.
    #[error("this computer asked for a {asked} frame, and the server answered {answered}")]
    Unexpected {
        answered: &'static str,
        asked: &'static str,
    },
    /// The link ended before the server answered.
    #[error("the link ended before the server answered the {asked}")]
    Ended { asked: &'static str },
    /// The device name breaks the rule.
    #[error("{0:?} is not one device name: 1 to 40 characters of a-z, 0-9 and -")]
    Device(String),
    /// The queue file cannot be read or written.
    #[error("{0}")]
    Queue(String),
    /// The server store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}
