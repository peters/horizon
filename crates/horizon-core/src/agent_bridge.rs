//! One agent program on a pipe, with no terminal.
//!
//! Horizon starts one agent program for one conversation and speaks its line
//! format: the person's messages leave on the standard input of the program as
//! one JSON line each, and the lines of its standard output become the small set
//! of typed events of [`BridgeEvent`]. The program draws no screen, so this
//! module reads no terminal, no cursor and no escape sequence.
//!
//! Every byte that the program writes to its standard output and to its standard
//! error is untrusted, as is every byte that it does not write. The module turns
//! a line into an event only when the line is JSON of the one grammar that
//! [`ClaudeCodec`] reads; every other line becomes [`BridgeEvent::Dropped`] and
//! ends nothing, except an over-long `result` line, which also ends its turn. No
//! text of the program reaches a log line, a path or a file of its own: the
//! events hold it, and the conversation of [`Conversation`] shows it as plain
//! text. One text of the program reaches a command line, and only as a UUID: the
//! session id of `--resume`. The page
//! `docs/review/trust-boundaries/agent-bridge.md` states that boundary, and
//! `docs/architecture/agent-bridge.md` gives the threads, the bounds and the
//! line format as it was measured on Claude Code 2.1.332.
//!
//! One [`Bridge`] owns one child and three threads: `agent-bridge-out` reads the
//! lines of the standard output with the bound of one line, `agent-bridge-in`
//! writes the queued lines and flushes them, and a reader of the standard error
//! keeps its last bytes. A frame calls [`Bridge::poll`] and never waits.
//!
//! The dock of the window is the one caller of [`Bridge::start`] and
//! [`Conversation`], and its worker thread runs [`store`], which keeps the
//! entries of a workspace across a restart. The card path and the choice of the
//! program for the person are later slices.

use std::time::Duration;
pub mod actor;
mod codec;
mod conversation;
mod ending;
mod event;
mod line_reader;
mod program;
pub mod store;
#[cfg(test)]
mod tests;
mod worker;

pub use codec::{ClaudeCodec, LineCodec, LineShape};
pub use conversation::{
    Conversation, ConversationSnapshot, Entry, LimitNote, Link, ProposalItem, ProposalState, ToolState,
};
pub use event::{BridgeEvent, DropReason};
pub use program::{
    BridgeIdentity, ENV_REMOVE_NAMES, ENV_REMOVE_PREFIXES, FRONT_DESK_MODEL, FRONT_DESK_MODEL_ENV, FRONT_DESK_PROMPT,
    NO_TOOLS, ProgramSpec, SessionChoice, claude, claude_for_front_desk, claude_for_workspace, front_desk_model,
};
pub use worker::{Bridge, BridgeError};

/// The bytes of one line of the program. A longer line is read to its newline,
/// dropped whole and reported as [`DropReason::TooLong`], and the reader parses
/// only the shape of its head (see [`LineShape`]) so that the end of a turn is
/// not lost.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// The bytes of one text of the person or of the agent.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;

/// The characters of one id that a line of the program names: the message id,
/// the tool id and the session id. A longer id is cut when it enters an event, so
/// the entries and the map of the codec hold no id above this bound.
pub const MAX_ID_CHARS: usize = 128;

/// The characters of one summary: of a tool call, of a tool result and of the
/// refusal of a tool.
pub const MAX_SUMMARY_CHARS: usize = 400;

/// The entries that one conversation keeps. Older entries are dropped, and one
/// [`Entry::Notice`] at the top of the list says how many.
pub const MAX_ENTRIES: usize = 2000;

/// The last bytes of the standard error of the program that the bridge keeps for
/// the notice of its end.
pub const STDERR_TAIL_BYTES: usize = 8 * 1024;

/// How long [`Bridge::stop`] may wait for the program and for its threads.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);
