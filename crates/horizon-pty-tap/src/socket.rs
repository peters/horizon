//! A terminal that runs in the holder of a panel instead of in this process.
//!
//! [`SocketPty`] is the socket of a holder in the shape of a PTY: the reader yields
//! the bytes of the `replay` and `output` frames as one plain stream, the writer
//! wraps input in `input` frames, and a new size becomes a `resize` frame. The exit
//! frame of the program reaches the terminal as the event of a local child, and the
//! end of the link ends the event loop with one error and never with a read of zero
//! bytes, which a socket at its end reports for ever.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use alacritty_terminal::event::{Event, OnResize, WindowSize};
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite};
use polling::{Event as Interest, PollMode, Poller};

use crate::frames::{self, Frame, Frames};
use crate::inbox::{StatusInbox, StatusTap};
use crate::reader::StatusReader;

/// The bytes of one read of the link.
const READ: usize = 16 * 1024;
/// The bytes of one size: rows, columns, cell width and cell height.
const SIZE: usize = 8;

/// How the link of a holder ended.
#[derive(Clone, Copy)]
enum Ending {
    /// The status of the exit frame of the program.
    Program(Option<i32>),
    /// No exit frame: the holder of the panel itself went away.
    Link,
}

/// The socket of a holder in the shape of a PTY.
pub struct SocketPty {
    reader: StatusReader<LinkReader>,
    writer: LinkWriter,
}

impl SocketPty {
    /// Makes a terminal of `stream`, the link of a holder that answered a hello, and
    /// asks that holder for the size of its terminal first, so the screen of its
    /// program starts at the size of the panel that shows it.
    ///
    /// # Errors
    ///
    /// Returns why the link takes no size.
    pub fn attach(
        stream: UnixStream,
        window_size: WindowSize,
        events: Sender<Event>,
    ) -> io::Result<(Self, StatusInbox)> {
        stream.set_nonblocking(true)?;
        let mut writer = LinkWriter {
            stream: stream.try_clone()?,
            frame: Vec::new(),
            sent: 0,
            length: 0,
            size: None,
        };
        writer.queue_size(window_size);
        let _ = writer.send(&[])?;
        let (tap, inbox) = StatusTap::new();
        Ok((
            Self {
                reader: StatusReader::new(LinkReader::new(stream, events), tap),
                writer,
            },
            inbox,
        ))
    }
}

impl EventedReadWrite for SocketPty {
    type Reader = StatusReader<LinkReader>;
    type Writer = LinkWriter;

    // SAFETY: the contract of this method is that the source it registers outlives
    // its registration in the poller. This registers the socket of the link, which
    // this `SocketPty` owns for its whole life, and the poller is dropped with the
    // event loop that owns this wrapper.
    #[allow(unsafe_code)]
    unsafe fn register(&mut self, poll: &Arc<Poller>, interest: Interest, mode: PollMode) -> io::Result<()> {
        // SAFETY: the caller of this method promises that what it registers
        // outlives the registration, and this call registers only the socket that
        // this value owns.
        unsafe { poll.add_with_mode(&self.reader.source().stream, interest, mode) }
    }

    fn reregister(&mut self, poll: &Arc<Poller>, interest: Interest, mode: PollMode) -> io::Result<()> {
        poll.modify_with_mode(&self.reader.source().stream, interest, mode)
    }

    fn deregister(&mut self, poll: &Arc<Poller>) -> io::Result<()> {
        poll.delete(&self.reader.source().stream)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }
}

impl EventedPty for SocketPty {
    /// A holder has no child of this process: the exit of its program reaches the
    /// terminal as [`Event::ChildExit`], which the reader of this link sends.
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        None
    }
}

impl OnResize for SocketPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.writer.queue_size(window_size);
        // A size that arrives while a frame is open reaches the holder with the next
        // write of this link, and a link that is gone reports its own end.
        if self.writer.sent == self.writer.frame.len() {
            let _ = self.writer.send(&[]);
        }
    }
}

/// The link of a holder, read as the plain output stream of its program.
pub struct LinkReader {
    stream: UnixStream,
    decoded: Frames,
    pending: Vec<u8>,
    ending: Option<Ending>,
    events: Sender<Event>,
}

impl LinkReader {
    fn new(stream: UnixStream, events: Sender<Event>) -> Self {
        Self {
            stream,
            decoded: Frames::default(),
            pending: Vec::new(),
            ending: None,
            events,
        }
    }

    /// Records the end of the link, which the next read reports once the output
    /// before it has reached the terminal.
    fn end_with(&mut self, ending: Ending) {
        self.ending.get_or_insert(ending);
    }

    /// Takes the frames of one read of the link: the payloads of its output, and
    /// the end that it reports.
    fn decode(&mut self, bytes: &[u8]) -> io::Result<()> {
        for frame in self
            .decoded
            .push(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        {
            match frame {
                Frame::Replay(payload) | Frame::Output(payload) => self.pending.extend_from_slice(&payload),
                Frame::Exit(status) => self.end_with(Ending::Program(status)),
                Frame::Refusal => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "the holder serves another client",
                    ));
                }
                other => return Err(io::Error::other(format!("the holder sent the frame {}", other.kind()))),
            }
        }
        Ok(())
    }

    /// Tells the terminal how the link ended, and answers the error that ends the
    /// event loop.
    fn ended(&mut self, ending: Ending) -> io::Error {
        let event = match ending {
            Ending::Program(Some(code)) => Event::ChildExit(ExitStatus::from_raw(code << 8)),
            Ending::Program(None) | Ending::Link => Event::Exit,
        };
        let _ = self.events.send(event);
        // The end of the link ends the loop of the terminal, which logs this error.
        match ending {
            Ending::Program(_) => io::Error::other("the program of this panel ended"),
            Ending::Link => io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the holder of this panel closed its link",
            ),
        }
    }
}

impl Read for LinkReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut chunk = [0u8; READ];
        loop {
            if !self.pending.is_empty() {
                let take = self.pending.len().min(buffer.len());
                buffer[..take].copy_from_slice(&self.pending[..take]);
                self.pending.drain(..take);
                return Ok(take);
            }
            if let Some(ending) = self.ending.take() {
                return Err(self.ended(ending));
            }
            match self.stream.read(&mut chunk) {
                // A socket at its end reports a read of zero bytes for ever.
                Ok(0) => self.end_with(Ending::Link),
                Ok(got) => self.decode(&chunk[..got])?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// The link of a holder, written as the input of its program.
///
/// The link is non-blocking, so one frame can take several writes: the writer
/// holds the frame in progress and answers the input bytes that the link took,
/// which the event loop gives back to it.
pub struct LinkWriter {
    stream: UnixStream,
    /// The frame in progress, header and payload, and how much of it the link took.
    frame: Vec<u8>,
    sent: usize,
    /// The input bytes of that frame, so that a frame of a size reports none.
    length: usize,
    /// The newest size of the terminal, until the link writes it.
    size: Option<[u8; SIZE]>,
}

impl LinkWriter {
    /// Keeps the newest size of the terminal, because a holder needs no older one.
    fn queue_size(&mut self, size: WindowSize) {
        self.size = Some(frames::size(
            size.num_lines,
            size.num_cols,
            size.cell_width,
            size.cell_height,
        ));
    }

    /// Writes what the link takes now, and answers the input bytes that it took.
    fn send(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.sent == self.frame.len() {
            self.frame.clear();
            self.sent = 0;
            self.length = 0;
            if let Some(size) = self.size.take() {
                self.frame = frames::frame_bytes(frames::SIZE_KIND, &size);
            } else if !bytes.is_empty() {
                self.length = bytes.len().min(frames::MAX_PAYLOAD as usize);
                self.frame = frames::frame_bytes(frames::INPUT, &bytes[..self.length]);
            }
        }
        if self.frame.is_empty() {
            return Ok(0);
        }
        let before = self.taken();
        match self.stream.write(&self.frame[self.sent..]) {
            Ok(written) => self.sent += written,
            // A link that asks to wait takes the frame with a later write.
            Err(error) if !matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => {
                return Err(error);
            }
            Err(_) => {}
        }
        Ok(self.taken() - before)
    }

    /// The input bytes of the frame in progress that the link took.
    fn taken(&self) -> usize {
        self.sent.saturating_sub(frames::HEADER).min(self.length)
    }
}

impl Write for LinkWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.send(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
