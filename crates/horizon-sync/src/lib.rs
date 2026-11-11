//! The Horizon server store, and the link that a computer uses to reach it.
//!
//! One server holds what more than one computer must agree on: today the machine
//! list, later the machine public lines, cards, clicks and waits. The server is
//! the only writer of that state. A computer never opens the server file: it
//! sends *intents* over a link, and the server applies them one at a time.
//!
//! The rules of this crate, in the words of the design page
//! `docs/architecture/horizon-server.md`:
//!
//! 1. One writer task applies intents. Each push is one short transaction in WAL
//!    mode with `synchronous=FULL`, and it holds no network call. Readers use
//!    their own read connections and never block the writer.
//! 2. Each committed change takes the next log index inside its transaction. The
//!    index has no gaps, and the state of a computer is the last index that it
//!    has.
//! 3. An intent carries an identifier from its computer, a device name and a
//!    counter. A repeated intent gets its first result again and changes nothing.
//! 4. An intent on a versioned object carries the version that the computer saw.
//!    The first intent on that version wins; a later one gets a typed refusal.
//! 5. The server stamps its own time on every change. A client's clock is never
//!    read.
//! 6. A computer keeps its unsent intents in order in a local queue file, and it
//!    resends them with the same identifiers.
//!
//! The store is the only writer and the only reader of the file. This slice has
//! no binary entry: the next one wires `ssh` and the flags, and the frames reach
//! it through [`serve`] over any pair of byte streams.

#![forbid(unsafe_code)]

mod client;
mod error;
mod frame;
mod intent;
mod link;
mod protocol;
mod server;
mod store;
mod tip;

pub use client::{Client, Link, LinkState, Mirror, ObjectMirror, PullOutcome, PushOutcome};
pub use error::{Refusal, StoreError, SyncError};
pub use frame::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};
pub use intent::{
    Intent, IntentResult, KIND_MACHINE, KIND_PROBE, MAX_INTENTS_PER_PUSH, MAX_PROBES, Op, Outcome, PushReply,
};
pub use link::{SocketError, SocketServer, relay, run_link_stdio_to_unix_socket, run_server_on_unix_socket};
pub use protocol::{
    Changes, Frame, Hello, HelloOk, LinkError, LinkErrorCode, PROTOCOL_VERSION, Pull, Push, Row, Wait,
    check_device_name,
};
pub use server::{MAX_OPEN_WAITS, WAIT_BOUND, serve};
pub use store::Store;
pub use tip::tip;
