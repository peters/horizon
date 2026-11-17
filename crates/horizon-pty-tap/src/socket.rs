//! A terminal that runs in the holder of a panel instead of in this process.
//!
//! [`LinkReader`] yields the bytes of the `replay` and `output` frames of a holder
//! as one plain stream, and [`LinkWriter`] wraps the input of a terminal in `input`
//! frames and a new size in a `resize` frame. The exit frame of the program reaches
//! the terminal as the event of a local child, and the end of the link ends the
//! event loop with one error and never with a read of zero bytes, which a socket at
//! its end reports for ever. [`AttachedLink`] holds the three of them for the PTY of
//! a panel that attaches.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::tty::ChildEvent;
use rustix::net::{RecvFlags, recv};

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

/// The link of a holder that is ready to serve the terminal of a panel.
pub(crate) struct AttachedLink {
    /// The socket that the event loop polls for the output of the program.
    pub(crate) source: UnixStream,
    /// The same socket, polled a second time for the end of the link.
    pub(crate) watch: UnixStream,
    pub(crate) reader: StatusReader<LinkReader>,
    pub(crate) writer: LinkWriter,
    /// The inbox of the program status of the program of this link.
    pub(crate) inbox: StatusInbox,
}

/// Writes the hello of `stream` as the answers of no holder: the terminal that
/// attaches reads that hello itself, and this link starts after it.
///
/// # Errors
///
/// Returns why the link takes no size.
pub(crate) fn attach(stream: UnixStream, window_size: WindowSize) -> io::Result<AttachedLink> {
    stream.set_nonblocking(true)?;
    let source = stream.try_clone()?;
    let watch = stream.try_clone()?;
    let mut writer = LinkWriter::of(stream.try_clone()?);
    // One row less and then the size of the panel, as a terminal multiplexer does
    // at an attach: the system tells a program only a size that changed, and a
    // program that draws with cursor moves repaints its screen at that signal.
    writer.resize(WindowSize {
        num_lines: window_size.num_lines.saturating_sub(1).max(1),
        ..window_size
    });
    writer.resize(window_size);
    let (tap, inbox) = StatusTap::new();
    Ok(AttachedLink {
        source,
        watch,
        reader: StatusReader::new(LinkReader::new(stream), tap),
        writer,
        inbox,
    })
}

/// The link of a holder, read as the plain output stream of its program.
pub struct LinkReader {
    stream: UnixStream,
    decoded: Frames,
    pending: Vec<u8>,
    ending: Option<Ending>,
}

impl LinkReader {
    fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            decoded: Frames::default(),
            pending: Vec::new(),
            ending: None,
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

    /// The end of the link, as the end of the child of a local terminal, once the
    /// holder ended its program or went away.
    ///
    /// The event loop of a terminal asks this from the key of the child of a PTY,
    /// which this link also registers, so no end of a link lets that loop spin.
    pub(crate) fn ended_child(&mut self) -> Option<ChildEvent> {
        let ending = self.ending.or_else(|| {
            let mut probe = [0u8; 1];
            match recv(&self.stream, &mut probe, RecvFlags::PEEK) {
                Ok((0, _)) => Some(Ending::Link),
                Ok(_) => None,
                Err(error) if io::Error::from(error).kind() == io::ErrorKind::WouldBlock => None,
                Err(_) => Some(Ending::Link),
            }
        })?;
        Some(ChildEvent::Exited(match ending {
            Ending::Program(status) => status.map(|code| ExitStatus::from_raw(code << 8)),
            Ending::Link => None,
        }))
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
            if self.ending.is_some() {
                // The link ends here, and the loop of the terminal must not read a
                // socket at its end again: the read of this link reports no bytes
                // and the key of its end reports the child event.
                let _ = self.stream.shutdown(Shutdown::Both);
                return Ok(0);
            }
            match self.stream.read(&mut chunk) {
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
    /// The writer of `stream`, whose frames start empty.
    pub(crate) fn of(stream: UnixStream) -> Self {
        Self {
            stream,
            frame: Vec::new(),
            sent: 0,
            length: 0,
            size: None,
        }
    }

    /// Keeps the newest size of the terminal, because a holder needs no older one,
    /// and writes it now when the link holds no frame.
    pub(crate) fn resize(&mut self, size: WindowSize) {
        self.size = Some(frames::size(
            size.num_lines,
            size.num_cols,
            size.cell_width,
            size.cell_height,
        ));
        // A size that arrives while a frame is open reaches the holder with the next
        // write of this link, and a link that is gone reports its own end.
        if self.sent == self.frame.len() {
            let _ = self.send(&[]);
        }
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

#[cfg(test)]
mod tests;
