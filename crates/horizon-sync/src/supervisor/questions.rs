//! The questions that the window asks the server, and the answers that return.
//!
//! A question travels on the open link of the supervisor, so a computer that
//! reaches its server over `ssh` asks the same questions as one that joined with
//! a code. The answer comes back on a channel that the caller polls, so no frame
//! of the window waits for the server.

use crate::protocol::{DeviceRow, Invited};

/// One question that the window or a worker asks the server.
///
/// The supervisor sends it on the open link, so a computer that reaches its
/// server over `ssh` can ask the same questions as one that joined it with a
/// code. The answer comes back on a channel that the caller polls, so no frame
/// waits for the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerRequest {
    /// One invitation for another computer.
    Invite,
    /// The list of the computers of this server.
    Devices,
    /// The removal of one computer.
    Remove(String),
}

/// The answer to one [`ServerRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerReply {
    /// One invitation, with its code, its link and the time that it still lives.
    Invited(Invited),
    /// The computers of this server, and the time of the server that answered.
    Devices {
        /// The rows, in name order.
        rows: Vec<DeviceRow>,
        /// The time of the server when it answered.
        now_millis: u64,
    },
    /// The removal of one computer: whether the server held it.
    Removed {
        /// The name of the computer.
        device: String,
        /// Whether the server held that computer.
        removed: bool,
    },
    /// The question did not reach its answer, with one sentence for the person.
    Failed(String),
}

/// The one sentence of a question that this computer cannot send, because its
/// link to the server is not there. The window reads it at once, so a button of
/// the Server tab never waits for a link that is away.
pub(super) const NO_LINK_SENTENCE: &str = "This computer has no link to the server now.";
