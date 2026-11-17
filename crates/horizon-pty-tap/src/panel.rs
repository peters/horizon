//! The PTY of a panel: a terminal of this process, or the socket of its holder.
//!
//! [`PanelPty`] forwards the three traits of the event loop of `alacritty_terminal`
//! to the variant of the panel, and it registers only the source that it owns. The
//! reader of either variant hands every byte of the program on unchanged while a
//! [`StatusReader`] scanner looks at it, so one pass over the output serves both the
//! VT parser and the status reports of the program.
//!
//! The interface of that trait cannot be implemented without one `unsafe`
//! function, because the trait declares `register` as an `unsafe fn` and the wrapper
//! must forward it. Nothing unsafe is added: the wrapper registers exactly the
//! sources of the variant that it owns and that live as long as it, and it registers
//! no source of its own.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use alacritty_terminal::event::{OnResize, WindowSize};
use alacritty_terminal::tty::{self, ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event, PollMode, Poller};

use crate::inbox::{StatusInbox, StatusTap};
use crate::reader::StatusReader;
use crate::share::ShareOutlet;
use crate::socket::{self, AttachedLink, LinkReader, LinkWriter};

/// The PTY of a panel, whose program runs in this process or in a holder.
pub struct PanelPty {
    /// The source that the event loop polls, and that reports the end of a child.
    source: Source,
    reader: PanelReader,
    writer: PanelWriter,
    /// The reports of the program status of the program of this panel.
    inbox: StatusInbox,
}

/// The source of the output of a panel.
enum Source {
    /// The master of the terminal of a program that this process started.
    Local(tty::Pty),
    /// The socket of the holder: its output, and the end of it.
    Attached { output: UnixStream, watch: UnixStream },
}

/// The key of the child event of a PTY in the event loop of `alacritty_terminal`,
/// which registers the terminal itself under key 0 (`tty/unix.rs`).
const CHILD_KEY: usize = 1;

impl PanelPty {
    /// The terminal of a program that this process starts.
    ///
    /// # Errors
    ///
    /// Returns why its reader cannot start.
    pub fn local(mut pty: tty::Pty) -> io::Result<Self> {
        let (tap, inbox) = StatusTap::new();
        let reader = StatusReader::new(pty.reader().try_clone()?, tap);
        let writer = pty.writer().try_clone()?;
        Ok(Self {
            source: Source::Local(pty),
            reader: PanelReader::Local(reader),
            writer: PanelWriter::Local(writer),
            inbox,
        })
    }

    /// The socket of the holder that runs the program of this panel.
    ///
    /// # Errors
    ///
    /// Returns why the link takes no size.
    pub fn attached(stream: UnixStream, window_size: WindowSize) -> io::Result<Self> {
        let AttachedLink {
            source,
            watch,
            reader,
            writer,
            inbox,
        } = socket::attach(stream, window_size)?;
        Ok(Self {
            source: Source::Attached { output: source, watch },
            reader: PanelReader::Attached(reader),
            writer: PanelWriter::Attached(writer),
            inbox,
        })
    }

    /// The reports of the program status of the program of this panel.
    #[must_use]
    pub fn inbox(&self) -> StatusInbox {
        self.inbox.clone()
    }

    /// The output of the program of this panel for a follower, which costs one
    /// atomic load for each read while nobody follows.
    #[must_use]
    pub fn share(&self) -> ShareOutlet {
        match &self.reader {
            PanelReader::Local(reader) => reader.share(),
            PanelReader::Attached(reader) => reader.share(),
        }
    }
}

impl EventedReadWrite for PanelPty {
    type Reader = PanelReader;
    type Writer = PanelWriter;

    // SAFETY: the contract of this method is that the source it registers outlives
    // its registration in the poller. This registers the source of the variant that
    // this panel holds, which lives as long as it, and the poller is dropped with
    // the event loop that owns this value.
    #[allow(unsafe_code)]
    unsafe fn register(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        match &mut self.source {
            // SAFETY: the caller of this method promises that what it registers
            // outlives the registration, and this registers only the terminal that
            // this value owns.
            Source::Local(pty) => unsafe { pty.register(poll, event, mode) },
            // SAFETY: the same promise holds for both registrations of the socket,
            // which this value owns. The socket carries the output of the program
            // under the key of this call and its end under the key of the child of a
            // PTY, so the loop reads one and ends on the other.
            Source::Attached { output, watch } => unsafe {
                poll.add_with_mode(&*output, event, mode)?;
                poll.add_with_mode(&*watch, Event::readable(CHILD_KEY), mode)
            },
        }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, event: Event, mode: PollMode) -> io::Result<()> {
        match &mut self.source {
            Source::Local(pty) => pty.reregister(poll, event, mode),
            Source::Attached { output, watch } => {
                poll.modify_with_mode(&*output, event, mode)?;
                poll.modify_with_mode(&*watch, Event::readable(CHILD_KEY), mode)
            }
        }
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        match &mut self.source {
            Source::Local(pty) => pty.deregister(poll),
            Source::Attached { output, watch } => {
                poll.delete(&*output)?;
                poll.delete(&*watch)
            }
        }
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }
}

impl EventedPty for PanelPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        match &mut self.source {
            Source::Local(pty) => pty.next_child_event(),
            // A holder has no child of this process: the end of its link answers the
            // exit frame of its program, or the end of the link itself.
            Source::Attached { .. } => match &mut self.reader {
                PanelReader::Attached(reader) => reader.source_mut().ended_child(),
                PanelReader::Local(_) => None,
            },
        }
    }
}

impl OnResize for PanelPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        match &mut self.source {
            Source::Local(pty) => pty.on_resize(window_size),
            Source::Attached { .. } => {
                if let PanelWriter::Attached(writer) = &mut self.writer {
                    writer.resize(window_size);
                }
            }
        }
    }
}

/// The reader of the PTY of a panel.
pub enum PanelReader {
    Local(StatusReader<File>),
    Attached(StatusReader<LinkReader>),
}

impl Read for PanelReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Local(reader) => match reader.read(buffer) {
                // The event loop takes the bytes of a program and the end of its
                // terminal in one pass, and an error on the end would discard the
                // bytes that it took before it. So the end of the output answers
                // what the end of a file answers, and those bytes reach the grid.
                Err(error) if ends_the_output(&error) => Ok(0),
                answer => answer,
            },
            Self::Attached(reader) => reader.read(buffer),
        }
    }
}

/// Whether `error` is the end of the output of the program of a terminal: the
/// system answers `EIO` when no process holds the client side of a terminal and
/// its buffer is empty.
fn ends_the_output(error: &io::Error) -> bool {
    error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error())
}

/// The writer of the PTY of a panel.
pub enum PanelWriter {
    Local(File),
    Attached(LinkWriter),
}

impl Write for PanelWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Local(writer) => writer.write(bytes),
            Self::Attached(writer) => writer.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Local(writer) => writer.flush(),
            Self::Attached(writer) => writer.flush(),
        }
    }
}
