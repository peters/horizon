//! Shared terminals: the frames of a live link, and the relay of a server.
//!
//! A computer opens a second link beside its sync link and turns it into a live
//! link with one request. It lists the terminal panels that it shares, and it
//! follows panels of other computers. The server is the hub: it passes the output
//! of a panel to the computers that follow it and what they type to its owner,
//! and it keeps all of that in memory only.

pub(crate) mod alive;
mod hub;
pub(crate) mod mirror;
mod outbox;
pub(crate) mod protocol;
mod relay;
#[cfg(test)]
mod tests;

pub(crate) use alive::Alive;
pub use horizon_core::session_mirror::{MirrorEntry, MirrorProgram, MirrorSession};
pub(crate) use hub::LiveHub;
pub use mirror::{MAX_MIRROR_ENTRIES, MAX_MIRROR_FOLLOWS, MAX_MIRROR_SESSIONS, MirrorComputer, MirrorList};
pub use protocol::{
    LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveRequest, LiveState, LiveWatcher,
    MAX_LIVE_FOLLOWS, MAX_LIVE_OUTPUT_BYTES, MAX_LIVE_PANELS, MAX_LIVE_TYPED_BYTES, MAX_LIVE_WATCHERS,
};
pub(crate) use relay::serve;
