//! The status protocol of a program, read from the output of the terminal it runs in.
//!
//! A program tells its terminal what it is doing with one escape sequence, the
//! Program Status Protocol (OSC 7501, revision 0.4): `ESC ] 7501 ; pairs ST`, with
//! `state=working`, `state=blocked:kind=permission`, `state=done`, `state=error`,
//! `state=idle` and `state=clear`. The VT parser of this workspace drops a private
//! OSC number, so this crate reads the bytes in the reader of the PTY instead.
//!
//! [`TapPty`] wraps a PTY: every call the terminal event loop makes reaches the PTY
//! it owns, and the reader shows every byte unchanged while a [`StatusReader`]
//! scanner looks at it. Each complete payload reaches a [`StatusInbox`] that the
//! event loop thread never waits on, and a frame reads the newest one.
//!
//! The bytes of a panel are untrusted input of the program that runs in it. The
//! scanner is bounded: it keeps at most [`scanner::MAX_PAYLOAD_BYTES`] bytes of one
//! payload, it drops a longer one whole, it allocates nothing, and it cannot block
//! or panic on any byte of a stream.
#![deny(unsafe_code)]

pub mod frames;
mod inbox;
mod panel;
mod pty;
mod reader;
mod scanner;
mod socket;

pub use inbox::{QUERY_PAYLOAD, QUEUE_DEPTH, StatusInbox, StatusTap};
pub use panel::{PanelPty, PanelReader, PanelWriter};
pub use pty::TapPty;
pub use reader::StatusReader;
pub use scanner::{MAX_PAYLOAD_BYTES, Payload, Scanner};
pub use socket::{LinkReader, LinkWriter};

#[cfg(test)]
mod cost_tests;
