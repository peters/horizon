//! A PTY whose reader reports the program status that its program writes.
//!
//! The event loop of `alacritty_terminal` reads a terminal through the trait
//! [`EventedReadWrite`], so a wrapper of the PTY can sit between the loop and the
//! PTY: the wrapper passes every call on to the PTY it owns, and its reader hands
//! the bytes on unchanged while a scanner looks at them.
//!
//! The interface of that trait cannot be implemented without one `unsafe`
//! function, because the trait declares `register` as an `unsafe fn` and the
//! wrapper must forward it. Nothing unsafe is added: the wrapper registers exactly
//! the sources of the PTY that it owns and that live as long as it, and it
//! registers no source of its own.

use std::fs::File;
use std::io;
use std::sync::Arc;

use alacritty_terminal::event::OnResize;
use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};

use crate::inbox::{StatusInbox, StatusTap};
use crate::reader::StatusReader;

/// A PTY that reports the program status of its program to a [`StatusInbox`].
pub struct TapPty<P> {
    pty: P,
    reader: StatusReader<File>,
}

impl<P> TapPty<P>
where
    P: EventedReadWrite<Reader = File, Writer = File>,
{
    /// Wraps `pty` and answers the inbox that its status reports reach.
    ///
    /// # Errors
    ///
    /// Returns the error of the clone of the PTY's file, which is the one read
    /// source of the wrapper: the event loop reads every byte through it, and it is
    /// the same open file as the PTY's own descriptor, so the bytes, the position
    /// and the end of the stream are the ones the PTY itself would give.
    pub fn new(mut pty: P) -> io::Result<(Self, StatusInbox)> {
        let (tap, inbox) = StatusTap::new();
        let file = pty.reader().try_clone()?;
        Ok((
            Self {
                pty,
                reader: StatusReader::new(file, tap),
            },
            inbox,
        ))
    }
}

impl<P> TapPty<P>
where
    P: EventedReadWrite<Reader = File, Writer = File>,
{
    /// The PTY and its reader, taken apart for the panel that forwards them.
    #[must_use]
    pub fn split(self) -> (P, StatusReader<File>) {
        (self.pty, self.reader)
    }
}

impl<P> EventedReadWrite for TapPty<P>
where
    P: EventedReadWrite<Reader = File, Writer = File>,
{
    type Reader = StatusReader<File>;
    type Writer = P::Writer;

    // SAFETY: the contract of this method is that the sources it registers outlive
    // their registration in the poller. This wrapper registers exactly the sources
    // of the PTY it owns, in one call that holds the same contract, and adds no
    // source of its own: the reader shares the open file of that PTY, which the
    // PTY keeps for the whole life of the wrapper, and the poller is dropped with
    // the event loop that owns this wrapper.
    #[allow(unsafe_code)]
    unsafe fn register(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        // SAFETY: the caller of this method promises that what it registers
        // outlives the registration; this call registers only the sources of the
        // owned PTY, which lives as long as this wrapper.
        unsafe { self.pty.register(poll, event, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        self.pty.reregister(poll, event, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        self.pty.deregister(poll)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.pty.writer()
    }
}

impl<P> EventedPty for TapPty<P>
where
    P: EventedPty<Reader = File, Writer = File>,
{
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.pty.next_child_event()
    }
}

impl<P: OnResize> OnResize for TapPty<P> {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.pty.on_resize(window_size);
    }
}
