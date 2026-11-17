//! The status protocol of a program, read from the output of the terminal it runs in.
//!
//! A program tells its terminal what it is doing with one escape sequence, the
//! Program Status Protocol (OSC 7501, revision 0.4): `ESC ] 7501 ; pairs ST`, with
//! `state=working`, `state=blocked:kind=permission`, `state=done`, `state=error`,
//! `state=idle` and `state=clear`. The VT parser of this workspace drops a private
//! OSC number, so this crate reads the bytes in the reader of the PTY instead.
//!
//! [`PanelPty`] wraps a PTY or the socket of a holder: every call the terminal
//! event loop makes reaches the source that the wrapper owns, and the reader shows
//! every byte unchanged while a `StatusReader` scanner looks at it. Each complete
//! payload reaches a [`StatusInbox`] that the event loop thread never waits on, and
//! a frame reads the newest one.
//!
//! The bytes of a panel are untrusted input of the program that runs in it. The
//! scanner is bounded: it keeps at most [`scanner::MAX_PAYLOAD_BYTES`] bytes of one
//! payload, it drops a longer one whole, it allocates nothing, and it cannot block
//! or panic on any byte of a stream.
//!
//! The same reader copies the output for a follower of the panel into a bounded
//! [`ShareOutlet`], and only while one follows a screen.
#![deny(unsafe_code)]

pub mod frames;
mod inbox;
mod panel;
mod reader;
mod scanner;
mod share;
mod socket;

pub use inbox::StatusInbox;
pub use panel::PanelPty;
pub use scanner::Payload;
pub use share::{ShareOutlet, ShareTake};

#[cfg(test)]
mod cost_tests;
