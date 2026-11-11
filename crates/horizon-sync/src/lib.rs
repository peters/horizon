//! The Horizon server store, and the link that a computer uses to reach it.
//!
//! One server holds what more than one computer must agree on: today the machine
//! list, the key that each machine uses, every machine key and every credential
//! value of the person, later cards, clicks and waits. The server is the only
//! writer of that state. A computer never opens the server file: it
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
//! A secret is one value of that state, and its body is sealed in a table of its
//! own with a key of the server home, so a copy of the database alone holds no
//! readable secret and the log row of a secret holds the kind, the id and the
//! version only. The computer keeps the id of a secret in its queue and reads the
//! body from its own key folder or credential store when it pushes.
//!
//! The store is the only writer and the only reader of the file. The frames reach
//! a server through [`serve`] over any pair of byte streams, and the two modes of
//! the binary run that over the standard input and output of one `ssh` child.

#![forbid(unsafe_code)]

mod client;
mod clock;
mod error;
mod frame;
mod host;
mod intent;
mod invite;
mod join;
mod link;
mod listener;
mod login;
mod protocol;
mod server;
mod store;
mod supervisor;
#[cfg(test)]
mod test_support;
mod tip;

pub use client::{Client, Link, LinkState, Mirror, ObjectMirror, PullOutcome, PushOutcome};
pub use clock::{Clock, SystemClock};
pub use error::{Refusal, StoreError, SyncError};
pub use frame::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};
pub use host::{
    EXIT_NO_SERVER, HostError, ServerHome, ServerLock, ServerOptions, link_stdio, link_streams, serve_until_stopped,
    serve_with_options, terminate_flag,
};
pub use intent::{
    Intent, IntentResult, KIND_BINDING, KIND_MACHINE, KIND_PROBE, KIND_SECRET, MAX_INTENTS_PER_PUSH, MAX_PROBES,
    MAX_SECRET_BYTES, MAX_SECRETS, MAX_SECRETS_PER_PUSH, Op, Outcome, PushReply, SecretBody,
};
pub use invite::{InviteError, SERVER_DEVICE, ask, first_invitation, greet, invitation_text};
pub use join::{JoinBounds, JoinError, join, join_with};
pub use link::{SocketError, SocketServer, relay, run_link_stdio_to_unix_socket, run_server_on_unix_socket};
pub use listener::{ListenError, ListenerInfo, TcpServer, run_server_on_tcp};
pub use login::{hash_code, hash_token, new_code, new_token};
pub use protocol::{
    Changes, DeviceList, DeviceLogin, DeviceRemove, DeviceRemoved, DeviceRow, Devices, Frame, Hello, HelloOk, Invite,
    Invited, Join, Joined, LinkError, LinkErrorCode, PROTOCOL_VERSION, Pull, Push, Row, Wait, check_device_name,
};
pub use server::{
    LinkRegistry, LinkTicket, LinkTransport, MAX_OPEN_WAITS, NoTransport, ServerInfo, WAIT_BOUND, WRONG_CODE_DELAY,
    serve, serve_ssh, serve_tcp,
};
pub use store::{
    INVITE_LIFETIME_MILLIS, JoinAnswer, MAX_OPEN_INVITES, MAX_WRONG_CODES, REMOVED_KEPT_MILLIS, Store, TokenLogin,
    UNJOINED_GRACE_MILLIS, WRONG_CODE_WINDOW_MILLIS,
};
pub use supervisor::{
    LinkBounds, LinkSnapshot, REQUEST_BOUND, ServerReply, ServerRequest, Supervisor, SupervisorConfig, SyncHandle,
    SyncedMachineStore, WAIT_ANSWER_BOUND, WAIT_ANSWER_MARGIN, proposed_device_name, stored_device_name,
    write_device_name,
};
pub use tip::tip;
